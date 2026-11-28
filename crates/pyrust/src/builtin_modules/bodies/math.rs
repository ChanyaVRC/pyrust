// `math` module — included into `pub mod math { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// This file is the thin Python-registration facade. Concrete function
// implementations and supporting numerical algorithms live in submodules under
// `bodies/math/`.
//
// Reference: <https://docs.python.org/3/library/math.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "math/coercion.rs"]
mod coercion;
#[path = "math/float_algorithms.rs"]
mod float_algorithms;
#[path = "math/floating.rs"]
mod floating;
#[path = "math/integer_algorithms.rs"]
mod integer_algorithms;
#[path = "math/number_theory.rs"]
mod number_theory;
#[path = "math/special_funcs.rs"]
mod special_funcs;
#[path = "math/special_functions.rs"]
mod special_functions;
#[path = "math/summation.rs"]
mod summation;
#[path = "math/trig_hyperbolic.rs"]
mod trig_hyperbolic;

use self::floating::{unary_checked, unary_overflow, unary_pred, unary_raw};

// The float kernels behind `math.sqrt` / `math.log` / ... and `math.lgamma`,
// shared with `random`'s native distributions so both report identical
// results and errors.
pub(crate) use self::float_algorithms::check_math_result;
pub(crate) use self::special_functions::m_lgamma;

pyrust_module! {
    constants {
        "pi"  => Value::float(std::f64::consts::PI),
        "e"   => Value::float(std::f64::consts::E),
        "tau" => Value::float(std::f64::consts::TAU),
        "inf" => Value::float(f64::INFINITY),
        "nan" => Value::float(f64::NAN),
    }

    fn floor(args) -> Result<Value> {
        number_theory::floor(_interp, FN_NAME, args)
    }

    fn ceil(args) -> Result<Value> {
        number_theory::ceil(_interp, FN_NAME, args)
    }

    fn trunc(args) -> Result<Value> {
        number_theory::trunc(_interp, FN_NAME, args)
    }

    fn sqrt(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::sqrt)
    }

    fn fabs(args) -> Result<Value> {
        unary_raw(_interp, FN_NAME, args, f64::abs)
    }

    fn sin(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::sin)
    }

    fn cos(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::cos)
    }

    fn tan(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::tan)
    }

    fn asin(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::asin)
    }

    fn acos(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::acos)
    }

    fn atan(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::atan)
    }

    fn exp(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::exp)
    }

    fn log2(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::log2)
    }

    fn log10(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::log10)
    }

    fn isnan(args) -> Result<Value> {
        unary_pred(_interp, FN_NAME, args, f64::is_nan)
    }

    fn isinf(args) -> Result<Value> {
        unary_pred(_interp, FN_NAME, args, f64::is_infinite)
    }

    fn pow(args) -> Result<Value> {
        floating::pow(_interp, FN_NAME, args)
    }

    fn atan2(args) -> Result<Value> {
        trig_hyperbolic::atan2(_interp, FN_NAME, args)
    }

    fn log(args) -> Result<Value> {
        floating::log(_interp, FN_NAME, args)
    }

    fn copysign(args) -> Result<Value> {
        floating::copysign(_interp, FN_NAME, args)
    }

    fn isfinite(args) -> Result<Value> {
        unary_pred(_interp, FN_NAME, args, f64::is_finite)
    }

    fn hypot(args) -> Result<Value> {
        floating::hypot(_interp, FN_NAME, args)
    }

    fn dist(args) -> Result<Value> {
        floating::dist(_interp, FN_NAME, args)
    }

    fn gcd(args) -> Result<Value> {
        number_theory::gcd(_interp, FN_NAME, args)
    }

    fn lcm(args) -> Result<Value> {
        number_theory::lcm(_interp, FN_NAME, args)
    }

    fn factorial(args) -> Result<Value> {
        number_theory::factorial(_interp, FN_NAME, args)
    }

    fn comb(args) -> Result<Value> {
        number_theory::comb(_interp, FN_NAME, args)
    }

    fn perm(args) -> Result<Value> {
        number_theory::perm(_interp, FN_NAME, args)
    }

    fn prod(args) -> Result<Value> {
        number_theory::prod(_interp, FN_NAME, args)
    }

    fn degrees(args) -> Result<Value> {
        unary_raw(_interp, FN_NAME, args, f64::to_degrees)
    }

    fn radians(args) -> Result<Value> {
        unary_raw(_interp, FN_NAME, args, f64::to_radians)
    }

    fn sinh(args) -> Result<Value> {
        unary_overflow(_interp, FN_NAME, args, f64::sinh)
    }

    fn cosh(args) -> Result<Value> {
        unary_overflow(_interp, FN_NAME, args, f64::cosh)
    }

    fn tanh(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::tanh)
    }

    fn asinh(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::asinh)
    }

    fn acosh(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::acosh)
    }

    fn atanh(args) -> Result<Value> {
        trig_hyperbolic::atanh(_interp, FN_NAME, args)
    }

    fn expm1(args) -> Result<Value> {
        unary_overflow(_interp, FN_NAME, args, f64::exp_m1)
    }

    fn log1p(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::ln_1p)
    }

    fn exp2(args) -> Result<Value> {
        unary_overflow(_interp, FN_NAME, args, f64::exp2)
    }

    fn cbrt(args) -> Result<Value> {
        unary_checked(_interp, FN_NAME, args, f64::cbrt)
    }

    fn fmod(args) -> Result<Value> {
        floating::fmod(_interp, FN_NAME, args)
    }

    fn remainder(args) -> Result<Value> {
        floating::remainder(_interp, FN_NAME, args)
    }

    fn modf(args) -> Result<Value> {
        floating::modf(_interp, FN_NAME, args)
    }

    fn frexp(args) -> Result<Value> {
        floating::frexp(_interp, FN_NAME, args)
    }

    fn ldexp(args) -> Result<Value> {
        floating::ldexp(_interp, FN_NAME, args)
    }

    fn nextafter(args) -> Result<Value> {
        floating::nextafter(_interp, FN_NAME, args)
    }

    fn ulp(args) -> Result<Value> {
        floating::ulp(_interp, FN_NAME, args)
    }

    fn isqrt(args) -> Result<Value> {
        number_theory::isqrt(_interp, FN_NAME, args)
    }

    fn isclose(args) -> Result<Value> {
        floating::isclose(_interp, FN_NAME, args)
    }

    fn fsum(args) -> Result<Value> {
        floating::fsum(_interp, FN_NAME, args)
    }

    fn sumprod(args) -> Result<Value> {
        floating::sumprod(_interp, FN_NAME, args)
    }

    fn gamma(args) -> Result<Value> {
        special_funcs::gamma(_interp, FN_NAME, args)
    }

    fn lgamma(args) -> Result<Value> {
        special_funcs::lgamma(_interp, FN_NAME, args)
    }

    fn erf(args) -> Result<Value> {
        special_funcs::erf(_interp, FN_NAME, args)
    }

    fn erfc(args) -> Result<Value> {
        special_funcs::erfc(_interp, FN_NAME, args)
    }
}
