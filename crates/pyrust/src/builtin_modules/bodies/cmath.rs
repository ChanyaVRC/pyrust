// `cmath` module — included into `pub mod cmath { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs` (issue #3185).
//
// Reference: <https://docs.python.org/3/library/cmath.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "cmath/coercion.rs"]
mod coercion;
#[path = "cmath/funcs.rs"]
mod funcs;

pyrust_module! {
    constants {
        "pi"   => Value::float(std::f64::consts::PI),
        "e"    => Value::float(std::f64::consts::E),
        "tau"  => Value::float(std::f64::consts::TAU),
        "inf"  => Value::float(f64::INFINITY),
        "infj" => Value::complex(0.0, f64::INFINITY),
        "nan"  => Value::float(f64::NAN),
        "nanj" => Value::complex(0.0, f64::NAN),
    }

    fn phase(args) -> Result<Value> {
        funcs::phase(_interp, FN_NAME, args)
    }

    fn polar(args) -> Result<Value> {
        funcs::polar(_interp, FN_NAME, args)
    }

    fn rect(args) -> Result<Value> {
        funcs::rect(_interp, FN_NAME, args)
    }

    fn exp(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_exp)
    }

    fn log(args) -> Result<Value> {
        funcs::log(_interp, FN_NAME, args)
    }

    fn log10(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_log10)
    }

    fn sqrt(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_sqrt)
    }

    fn acos(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_acos)
    }

    fn asin(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_asin)
    }

    fn atan(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_atan)
    }

    fn cos(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_cos)
    }

    fn sin(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_sin)
    }

    fn tan(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_tan)
    }

    fn acosh(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_acosh)
    }

    fn asinh(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_asinh)
    }

    fn atanh(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_atanh)
    }

    fn cosh(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_cosh)
    }

    fn sinh(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_sinh)
    }

    fn tanh(args) -> Result<Value> {
        funcs::unary_complex(_interp, FN_NAME, args, funcs::c_tanh)
    }

    fn isfinite(args) -> Result<Value> {
        funcs::isfinite(_interp, FN_NAME, args)
    }

    fn isinf(args) -> Result<Value> {
        funcs::isinf(_interp, FN_NAME, args)
    }

    fn isnan(args) -> Result<Value> {
        funcs::isnan(_interp, FN_NAME, args)
    }

    fn isclose(args) -> Result<Value> {
        funcs::isclose(_interp, FN_NAME, args)
    }
}
