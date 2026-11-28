//! The arithmetic the distributions of `Lib/random.py` are written in.
//!
//! Every distribution is implemented once, generically over [`Arith`], in the
//! evaluation order of the CPython source.  Two backends run it:
//!
//! * [`FloatOps`] — native `f64` with Python's `float` semantics (the
//!   `ZeroDivisionError` / `math domain error` checks, `float.__pow__`'s
//!   special cases, floor-modulo).  Used when every parameter is an exact
//!   `float` (or a small `int` / `bool`, which converts without rounding) and
//!   `self.random()` is a native generator, so no Python code can observe the
//!   difference.
//! * [`ValueOps`] — the interpreter's operators on Python objects and the
//!   `math` module's functions, for every other parameter type (`Fraction`,
//!   `Decimal`, a subclass overriding `random()`, ...), exactly like the
//!   Python source.

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{Value, ValueKind};

use super::dispatch::RandomSrc;
use super::support::{binop, call, compare, truthy, try_swallow};

#[derive(Clone, Copy)]
pub(super) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

#[derive(Clone, Copy)]
pub(super) enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

/// The `math` functions `Lib/random.py` imports.
#[derive(Clone, Copy)]
pub(super) enum MathFn {
    Log,
    Exp,
    Sqrt,
    Cos,
    Sin,
    Acos,
    Log2,
    Fabs,
    Lgamma,
}

impl MathFn {
    fn builtin_name(self) -> &'static str {
        match self {
            MathFn::Log => "math.log",
            MathFn::Exp => "math.exp",
            MathFn::Sqrt => "math.sqrt",
            MathFn::Cos => "math.cos",
            MathFn::Sin => "math.sin",
            MathFn::Acos => "math.acos",
            MathFn::Log2 => "math.log2",
            MathFn::Fabs => "math.fabs",
            MathFn::Lgamma => "math.lgamma",
        }
    }

    /// The `math` function on a float, with the module's result checks.
    pub(super) fn apply(self, x: f64) -> Result<f64> {
        use crate::builtin_modules::math::{check_math_result, m_lgamma};
        match self {
            MathFn::Log => check_math_result(x, x.ln()),
            MathFn::Exp => check_math_result(x, x.exp()),
            MathFn::Sqrt => check_math_result(x, x.sqrt()),
            MathFn::Cos => check_math_result(x, x.cos()),
            MathFn::Sin => check_math_result(x, x.sin()),
            MathFn::Acos => check_math_result(x, x.acos()),
            MathFn::Log2 => check_math_result(x, x.log2()),
            MathFn::Fabs => Ok(x.abs()),
            MathFn::Lgamma => m_lgamma(x),
        }
    }
}

/// Numbers and operations a distribution is computed with.
pub(super) trait Arith {
    type N: Clone;

    /// A `float` literal.
    fn lit(&self, x: f64) -> Self::N;
    /// An `int` literal.
    fn int(&self, x: i64) -> Self::N;
    /// `self.random()`.
    fn random(&mut self) -> Result<Self::N>;
    fn bin(&mut self, a: &Self::N, op: Op, b: &Self::N) -> Result<Self::N>;
    fn neg(&mut self, a: &Self::N) -> Result<Self::N>;
    fn cmp(&mut self, a: &Self::N, op: Cmp, b: &Self::N) -> Result<bool>;
    fn truthy(&mut self, a: &Self::N) -> Result<bool>;
    fn math(&mut self, f: MathFn, a: &Self::N) -> Result<Self::N>;
    /// `math.floor(a)` (an `int`).
    fn floor(&mut self, a: &Self::N) -> Result<Self::N>;

    /// `try: f() except ZeroDivisionError: None`.
    fn catch_zero_division<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<Option<T>>
    where
        Self: Sized,
    {
        Ok(try_swallow(|| f(self), |err| err.class_name_is("ZeroDivisionError"))?.ok())
    }
}

fn zero_division(msg: &str) -> PyError {
    PyError::named("ZeroDivisionError", msg.to_string())
}

/// `float_rem`: the remainder takes the sign of the divisor.
fn float_mod(a: f64, b: f64) -> Result<f64> {
    if b == 0.0 {
        return Err(zero_division("float modulo"));
    }
    let m = a % b;
    Ok(if m != 0.0 {
        if (b < 0.0) != (m < 0.0) { m + b } else { m }
    } else {
        0.0_f64.copysign(b)
    })
}

fn is_odd_integer(x: f64) -> bool {
    x.is_finite() && x % 2.0 != 0.0 && x.fract() == 0.0
}

/// `float_pow` (`float ** float`).  The cases CPython hands to `complex`
/// (a negative base with a fractional exponent) or reports through `errno`
/// (overflow) go through the interpreter so the result or the error matches.
fn float_pow(interp: &mut Interpreter, v: f64, w: f64) -> Result<f64> {
    if w == 0.0 {
        return Ok(1.0);
    }
    if v.is_nan() {
        return Ok(v);
    }
    if w.is_nan() {
        return Ok(if v == 1.0 { 1.0 } else { w });
    }
    if w.is_infinite() {
        let av = v.abs();
        return Ok(if av == 1.0 {
            1.0
        } else if (w > 0.0) == (av > 1.0) {
            w.abs()
        } else {
            0.0
        });
    }
    if v.is_infinite() {
        let odd = is_odd_integer(w);
        return Ok(if w > 0.0 {
            if odd { v } else { v.abs() }
        } else if odd {
            0.0_f64.copysign(v)
        } else {
            0.0
        });
    }
    if v == 0.0 {
        if w < 0.0 {
            return Err(zero_division("0.0 cannot be raised to a negative power"));
        }
        return Ok(if is_odd_integer(w) { v } else { 0.0 });
    }
    let mut base = v;
    let mut negate = false;
    if v < 0.0 {
        if w != w.floor() {
            return delegated_pow(interp, v, w);
        }
        base = -v;
        negate = is_odd_integer(w);
    }
    if base == 1.0 {
        return Ok(if negate { -1.0 } else { 1.0 });
    }
    let x = base.powf(w);
    if x.is_infinite() {
        return delegated_pow(interp, v, w);
    }
    Ok(if negate { -x } else { x })
}

fn delegated_pow(interp: &mut Interpreter, v: f64, w: f64) -> Result<f64> {
    let result = binop(interp, &Value::float(v), BinaryOp::Pow, &Value::float(w))?;
    match result.kind() {
        ValueKind::Float(f) => Ok(f),
        _ => Err(PyError::runtime(
            "random: float power produced a non-float result".to_string(),
        )),
    }
}

/// `math.floor` of a float, as a float holding the integer.
fn float_floor(x: f64) -> Result<f64> {
    if x.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "cannot convert float NaN to integer".to_string(),
        ));
    }
    if x.is_infinite() {
        return Err(PyError::named(
            "OverflowError",
            "cannot convert float infinity to integer".to_string(),
        ));
    }
    Ok(x.floor())
}

/// Native `f64` arithmetic over a native generator.
pub(super) struct FloatOps<'a> {
    pub(super) interp: &'a mut Interpreter,
    pub(super) this: &'a Value,
    pub(super) src: &'a RandomSrc,
}

impl Arith for FloatOps<'_> {
    type N = f64;

    fn lit(&self, x: f64) -> f64 {
        x
    }

    fn int(&self, x: i64) -> f64 {
        x as f64
    }

    fn random(&mut self) -> Result<f64> {
        self.src.draw_f64(self.this)
    }

    fn bin(&mut self, a: &f64, op: Op, b: &f64) -> Result<f64> {
        let (a, b) = (*a, *b);
        match op {
            Op::Add => Ok(a + b),
            Op::Sub => Ok(a - b),
            Op::Mul => Ok(a * b),
            Op::Div => {
                if b == 0.0 {
                    Err(zero_division("float division by zero"))
                } else {
                    Ok(a / b)
                }
            }
            Op::Mod => float_mod(a, b),
            Op::Pow => float_pow(self.interp, a, b),
        }
    }

    fn neg(&mut self, a: &f64) -> Result<f64> {
        Ok(-*a)
    }

    fn cmp(&mut self, a: &f64, op: Cmp, b: &f64) -> Result<bool> {
        Ok(match op {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
            Cmp::Eq => a == b,
        })
    }

    fn truthy(&mut self, a: &f64) -> Result<bool> {
        Ok(*a != 0.0)
    }

    fn math(&mut self, f: MathFn, a: &f64) -> Result<f64> {
        f.apply(*a)
    }

    fn floor(&mut self, a: &f64) -> Result<f64> {
        float_floor(*a)
    }
}

/// Python objects through the interpreter.
pub(super) struct ValueOps<'a> {
    pub(super) interp: &'a mut Interpreter,
    pub(super) this: &'a Value,
    pub(super) src: &'a RandomSrc,
}

fn binary_op(op: Op) -> BinaryOp {
    match op {
        Op::Add => BinaryOp::Add,
        Op::Sub => BinaryOp::Sub,
        Op::Mul => BinaryOp::Mul,
        Op::Div => BinaryOp::Div,
        Op::Mod => BinaryOp::Mod,
        Op::Pow => BinaryOp::Pow,
    }
}

fn compare_op(op: Cmp) -> BinaryOp {
    match op {
        Cmp::Lt => BinaryOp::Lt,
        Cmp::Le => BinaryOp::Le,
        Cmp::Gt => BinaryOp::Gt,
        Cmp::Ge => BinaryOp::Ge,
        Cmp::Eq => BinaryOp::Eq,
    }
}

impl Arith for ValueOps<'_> {
    type N = Value;

    fn lit(&self, x: f64) -> Value {
        Value::float(x)
    }

    fn int(&self, x: i64) -> Value {
        Value::int(x)
    }

    fn random(&mut self) -> Result<Value> {
        self.src.draw(self.interp, self.this)
    }

    fn bin(&mut self, a: &Value, op: Op, b: &Value) -> Result<Value> {
        binop(self.interp, a, binary_op(op), b)
    }

    fn neg(&mut self, a: &Value) -> Result<Value> {
        self.interp.eval_unary(a.clone(), crate::ast::UnaryOp::Neg)
    }

    fn cmp(&mut self, a: &Value, op: Cmp, b: &Value) -> Result<bool> {
        compare(self.interp, a, compare_op(op), b)
    }

    fn truthy(&mut self, a: &Value) -> Result<bool> {
        truthy(self.interp, a)
    }

    fn math(&mut self, f: MathFn, a: &Value) -> Result<Value> {
        call(
            self.interp,
            Value::builtin_function(f.builtin_name()),
            std::slice::from_ref(a),
        )
    }

    fn floor(&mut self, a: &Value) -> Result<Value> {
        call(
            self.interp,
            Value::builtin_function("math.floor"),
            std::slice::from_ref(a),
        )
    }
}
