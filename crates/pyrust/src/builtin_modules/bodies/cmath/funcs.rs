//! Complex elementary functions and special-value tables matching CPython 3.12's
//! `Modules/cmathmodule.c` (C99 Annex G branch cuts and signed-zero semantics).

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, LN_2, LN_10, PI};

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::Value;

use super::coercion::{cmath_arg_to_complex, cmath_arg_to_float, single_complex, two_floats};

const P34: f64 = 3.0 * FRAC_PI_4;
const INF: f64 = f64::INFINITY;
const NINF: f64 = f64::NEG_INFINITY;
const NAN: f64 = f64::NAN;

// Thresholds matching CPython's `Modules/cmathmodule.c` (IEEE 754 binary64).
// CM_LARGE_DOUBLE = DBL_MAX / 4.0
const CM_LARGE_DOUBLE: f64 = f64::MAX / 4.0;
// CM_SQRT_LARGE_DOUBLE = sqrt(CM_LARGE_DOUBLE)
const CM_SQRT_LARGE_DOUBLE: f64 = 6.703_903_964_971_298e153;
// CM_LOG_LARGE_DOUBLE = log(CM_LARGE_DOUBLE)
const CM_LOG_LARGE_DOUBLE: f64 = 708.396_418_532_264_1;
// CM_SQRT_DBL_MIN = sqrt(DBL_MIN)
const CM_SQRT_DBL_MIN: f64 = 1.491_668_146_240_041_3e-154;
// CM_SCALE_UP = 2*(DBL_MANT_DIG/2) + 1 = 55; 2^55 for subnormal scaling in sqrt.
// Actually CPython uses DBL_MAX_EXP / 2 etc., we use ldexp scaling via powers of 2.
const TWO_54: f64 = 18_014_398_509_481_984.0; // 2^54
const TWO_NEG_27: f64 = 1.0 / 134_217_728.0; // 2^-27

#[derive(Copy, Clone, Debug)]
pub(super) struct C {
    pub(super) re: f64,
    pub(super) im: f64,
}

impl C {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum MathErr {
    Domain,
    Overflow,
}

fn domain_err() -> PyError {
    PyError::named("ValueError", "math domain error".to_string())
}

fn overflow_err() -> PyError {
    PyError::named("OverflowError", "math range error".to_string())
}

fn to_py_err(e: MathErr) -> PyError {
    match e {
        MathErr::Domain => domain_err(),
        MathErr::Overflow => overflow_err(),
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SpecialType {
    NInf = 0,
    Neg = 1,
    NZero = 2,
    PZero = 3,
    Pos = 4,
    PInf = 5,
    Nan = 6,
}

fn classify(x: f64) -> SpecialType {
    if x.is_nan() {
        SpecialType::Nan
    } else if x.is_infinite() {
        if x.is_sign_negative() {
            SpecialType::NInf
        } else {
            SpecialType::PInf
        }
    } else if x == 0.0 {
        if x.is_sign_negative() {
            SpecialType::NZero
        } else {
            SpecialType::PZero
        }
    } else if x < 0.0 {
        SpecialType::Neg
    } else {
        SpecialType::Pos
    }
}

#[inline]
fn is_special(z: C) -> bool {
    !z.re.is_finite() || !z.im.is_finite()
}

/// Complex division matching CPython's `_Py_c_quot` (Smith's scaling algorithm).
fn c_quot(a: C, b: C) -> std::result::Result<C, MathErr> {
    let abs_breal = b.re.abs();
    let abs_bimag = b.im.abs();
    if abs_breal >= abs_bimag {
        if abs_breal == 0.0 {
            return Err(MathErr::Domain);
        }
        let ratio = b.im / b.re;
        let denom = b.re + b.im * ratio;
        Ok(C::new(
            (a.re + a.im * ratio) / denom,
            (a.im - a.re * ratio) / denom,
        ))
    } else if abs_bimag >= abs_breal {
        let ratio = b.re / b.im;
        let denom = b.re * ratio + b.im;
        Ok(C::new(
            (a.re * ratio + a.im) / denom,
            (a.im * ratio - a.re) / denom,
        ))
    } else {
        // At least one of b.re / b.im is NaN.
        Ok(C::new(NAN, NAN))
    }
}

/// Complex `sqrt(z)` matching CPython 3.12's `c_sqrt`.
pub(super) fn c_sqrt(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        if z.im.is_infinite() {
            return Ok(C::new(INF, z.im));
        }
        if z.re.is_infinite() {
            if z.re.is_sign_negative() {
                if z.im.is_nan() {
                    return Ok(C::new(NAN, INF));
                }
                return Ok(C::new(0.0, INF.copysign(z.im)));
            } else {
                if z.im.is_nan() {
                    return Ok(C::new(INF, NAN));
                }
                return Ok(C::new(INF, 0.0_f64.copysign(z.im)));
            }
        }
        // Remaining special cases involve NaN.
        return Ok(C::new(NAN, NAN));
    }

    if z.re == 0.0 && z.im == 0.0 {
        return Ok(C::new(0.0, z.im));
    }

    let ax = z.re.abs();
    let ay = z.im.abs();

    // Scale to avoid overflow or underflow during intermediate calculations.
    let s = if ax < CM_SQRT_DBL_MIN && ay < CM_SQRT_DBL_MIN {
        let sx = ax * TWO_54;
        let sy = ay * TWO_54;
        ((sx + sx.hypot(sy)) * 0.5).sqrt() * TWO_NEG_27
    } else {
        let ax8 = ax / 8.0;
        let ay8 = ay / 8.0;
        2.0 * (ax8 + ax8.hypot(ay8)).sqrt()
    };

    let d = ay / (2.0 * s);

    if z.re >= 0.0 {
        Ok(C::new(s, d.copysign(z.im)))
    } else {
        Ok(C::new(d, s.copysign(z.im)))
    }
}

/// Complex `exp(z)` matching CPython 3.12's `c_exp`.
pub(super) fn c_exp(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        if z.re.is_infinite() {
            if z.re.is_sign_negative() {
                if z.im.is_finite() {
                    let c = z.im.cos();
                    let s = z.im.sin();
                    return Ok(C::new(0.0_f64.copysign(c), 0.0_f64.copysign(s)));
                }
                return Ok(C::new(0.0, 0.0));
            } else {
                // +inf
                if z.im == 0.0 {
                    return Ok(C::new(INF, z.im));
                }
                if z.im.is_nan() {
                    return Ok(C::new(INF, NAN));
                }
                if z.im.is_infinite() {
                    return Err(MathErr::Domain);
                }
                let c = z.im.cos();
                let s = z.im.sin();
                return Ok(C::new(INF.copysign(c), INF.copysign(s)));
            }
        }
        if z.re.is_nan() {
            if z.im == 0.0 {
                return Ok(C::new(NAN, z.im));
            }
            return Ok(C::new(NAN, NAN));
        }
        // z.re is finite, z.im is inf or nan
        if z.im.is_infinite() {
            return Err(MathErr::Domain);
        }
        return Ok(C::new(NAN, NAN));
    }

    let r = if z.re > CM_LOG_LARGE_DOUBLE {
        // Avoid premature overflow when cos(y) or sin(y) is small.
        let l = (z.re - 1.0).exp();
        let re = l * z.im.cos() * std::f64::consts::E;
        let im = l * z.im.sin() * std::f64::consts::E;
        C::new(re, im)
    } else {
        let l = z.re.exp();
        C::new(l * z.im.cos(), l * z.im.sin())
    };

    if r.re.is_infinite() || r.im.is_infinite() {
        return Err(MathErr::Overflow);
    }
    Ok(r)
}

/// Complex `log(z)` matching CPython 3.12's `c_log`.
pub(super) fn c_log(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        let im = z.im.atan2(z.re);
        if z.re.is_infinite() || z.im.is_infinite() {
            return Ok(C::new(INF, im));
        }
        return Ok(C::new(NAN, NAN));
    }

    if z.re == 0.0 && z.im == 0.0 {
        return Err(MathErr::Domain);
    }

    let ax = z.re.abs();
    let ay = z.im.abs();

    let re = if ax > CM_LARGE_DOUBLE || ay > CM_LARGE_DOUBLE {
        (ax * 0.5).hypot(ay * 0.5).ln() + LN_2
    } else if ax < CM_SQRT_DBL_MIN && ay < CM_SQRT_DBL_MIN {
        (ax * TWO_54).hypot(ay * TWO_54).ln() - 54.0 * LN_2
    } else {
        let h = ax.hypot(ay);
        if (0.71..=1.73).contains(&h) {
            let am = ax.max(ay);
            let an = ax.min(ay);
            ((am - 1.0) * (am + 1.0) + an * an).ln_1p() * 0.5
        } else {
            h.ln()
        }
    };

    let im = z.im.atan2(z.re);
    Ok(C::new(re, im))
}

/// Complex `log10(z)` matching CPython 3.12's `c_log10`.
pub(super) fn c_log10(z: C) -> std::result::Result<C, MathErr> {
    let r = c_log(z)?;
    Ok(C::new(r.re / LN_10, r.im / LN_10))
}

/// Complex `cosh(z)` matching CPython 3.12's `c_cosh`.
pub(super) fn c_cosh(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        if z.re.is_infinite() {
            if z.im.is_infinite() {
                return Err(MathErr::Domain);
            }
            if z.im.is_nan() {
                return Ok(C::new(INF, NAN));
            }
            if z.im == 0.0 {
                let im = 0.0_f64.copysign(if z.re.is_sign_negative() { -z.im } else { z.im });
                return Ok(C::new(INF, im));
            }
            let c = z.im.cos();
            let s = z.im.sin();
            return Ok(C::new(INF.copysign(c), z.re * s));
        }
        if z.re.is_nan() {
            if z.im == 0.0 {
                return Ok(C::new(NAN, 0.0));
            }
            return Ok(C::new(NAN, NAN));
        }
        // z.re is finite, z.im is inf or nan
        if z.im.is_infinite() {
            return Err(MathErr::Domain);
        }
        // z.im is NaN
        if z.re == 0.0 {
            return Ok(C::new(NAN, 0.0));
        }
        return Ok(C::new(NAN, NAN));
    }

    let r = if z.re.abs() > CM_LOG_LARGE_DOUBLE {
        let x_minus_one = z.re - 1.0_f64.copysign(z.re);
        let re = x_minus_one.cosh() * z.im.cos() * std::f64::consts::E;
        let im = x_minus_one.sinh() * z.im.sin() * std::f64::consts::E;
        C::new(re, im)
    } else {
        let re = z.re.cosh() * z.im.cos();
        let im = z.re.sinh() * z.im.sin();
        C::new(re, im)
    };

    if r.re.is_infinite() || r.im.is_infinite() {
        return Err(MathErr::Overflow);
    }
    Ok(r)
}

/// Complex `sinh(z)` matching CPython 3.12's `c_sinh`.
pub(super) fn c_sinh(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        if z.re.is_infinite() {
            if z.im.is_infinite() {
                return Err(MathErr::Domain);
            }
            if z.im.is_nan() {
                return Ok(C::new(INF, NAN));
            }
            if z.im == 0.0 {
                return Ok(C::new(z.re, z.im));
            }
            let c = z.im.cos();
            let s = z.im.sin();
            return Ok(C::new(z.re * c, INF.copysign(s)));
        }
        if z.re.is_nan() {
            if z.im == 0.0 {
                return Ok(C::new(NAN, z.im));
            }
            return Ok(C::new(NAN, NAN));
        }
        // z.re is finite, z.im is inf or nan
        if z.im.is_infinite() {
            return Err(MathErr::Domain);
        }
        // z.im is NaN
        if z.re == 0.0 {
            return Ok(C::new(0.0, NAN));
        }
        return Ok(C::new(NAN, NAN));
    }

    let r = if z.re.abs() > CM_LOG_LARGE_DOUBLE {
        let x_minus_one = z.re - 1.0_f64.copysign(z.re);
        let re = x_minus_one.sinh() * z.im.cos() * std::f64::consts::E;
        let im = x_minus_one.cosh() * z.im.sin() * std::f64::consts::E;
        C::new(re, im)
    } else {
        let re = z.re.sinh() * z.im.cos();
        let im = z.re.cosh() * z.im.sin();
        C::new(re, im)
    };

    if r.re.is_infinite() || r.im.is_infinite() {
        return Err(MathErr::Overflow);
    }
    Ok(r)
}

/// Complex `tanh(z)` matching CPython 3.12's `c_tanh`.
pub(super) fn c_tanh(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        if z.re.is_infinite() {
            let re = 1.0_f64.copysign(z.re);
            let im = if z.im.is_finite() {
                if z.im == 0.0 {
                    z.im
                } else {
                    0.0_f64.copysign((2.0 * z.im).sin())
                }
            } else {
                0.0
            };
            return Ok(C::new(re, im));
        }
        if z.re.is_nan() {
            if z.im == 0.0 {
                return Ok(C::new(NAN, z.im));
            }
            return Ok(C::new(NAN, NAN));
        }
        // z.re is finite, z.im is inf or nan
        if z.im.is_infinite() {
            return Err(MathErr::Domain);
        }
        return Ok(C::new(NAN, NAN));
    }

    if z.re.abs() > CM_LOG_LARGE_DOUBLE {
        let re = 1.0_f64.copysign(z.re);
        let im = 4.0 * z.im.sin() * z.im.cos() * (-2.0 * z.re.abs()).exp();
        return Ok(C::new(re, im));
    }

    let tx = z.re.tanh();
    let ty = z.im.tan();
    let cx = 1.0 / z.re.cosh();
    let txty = tx * ty;
    let denom = 1.0 + txty * txty;
    let re = tx * (1.0 + ty * ty) / denom;
    let im = ((ty / denom) * cx) * cx;
    Ok(C::new(re, im))
}

/// Complex `acos(z)` matching CPython 3.12's `c_acos` (Hull, Fairgrieve & Tang, 1997).
pub(super) fn c_acos(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        let (rx, ry) = (classify(z.re), classify(z.im));
        use SpecialType::*;
        let res = match (rx, ry) {
            (NInf, NInf) => C::new(P34, INF),
            (NInf, Neg | NZero) => C::new(PI, INF),
            (NInf, PZero | Pos) => C::new(PI, NINF),
            (NInf, PInf) => C::new(P34, NINF),
            (NInf, Nan) => C::new(NAN, INF),
            (PInf, NInf) => C::new(FRAC_PI_4, INF),
            (PInf, Neg | NZero) => C::new(0.0, INF),
            (PInf, PZero | Pos) => C::new(0.0, NINF),
            (PInf, PInf) => C::new(FRAC_PI_4, NINF),
            (PInf, Nan) => C::new(NAN, INF),
            (Neg | NZero | PZero | Pos, NInf) => C::new(FRAC_PI_2, INF),
            (Neg | NZero | PZero | Pos, PInf) => C::new(FRAC_PI_2, NINF),
            (NZero | PZero, Nan) => C::new(FRAC_PI_2, NAN),
            (Nan, NInf) => C::new(NAN, INF),
            (Nan, PInf) => C::new(NAN, NINF),
            _ => C::new(NAN, NAN),
        };
        return Ok(res);
    }

    let (re, im) = if z.re.abs() > CM_LARGE_DOUBLE || z.im.abs() > CM_LARGE_DOUBLE {
        let re = z.im.abs().atan2(z.re);
        let mag = (z.re * 0.5).hypot(z.im * 0.5).ln() + 2.0 * LN_2;
        (re, (-mag).copysign(-z.im))
    } else {
        let s1 = c_sqrt(C::new(1.0 - z.re, -z.im))?;
        let s2 = c_sqrt(C::new(1.0 + z.re, z.im))?;
        let re = 2.0 * s1.re.atan2(s2.re);
        let im = (s2.re * s1.im - s2.im * s1.re).asinh();
        (re, im)
    };
    Ok(C::new(re, im))
}

/// Complex `acosh(z)` matching CPython 3.12's `c_acosh`.
pub(super) fn c_acosh(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        let (rx, ry) = (classify(z.re), classify(z.im));
        use SpecialType::*;
        let res = match (rx, ry) {
            (NInf, NInf) => C::new(INF, -P34),
            (NInf, Neg | NZero) => C::new(INF, -PI),
            (NInf, PZero | Pos) => C::new(INF, PI),
            (NInf, PInf) => C::new(INF, P34),
            (NInf, Nan) => C::new(INF, NAN),
            (PInf, NInf) => C::new(INF, -FRAC_PI_4),
            (PInf, Neg | NZero) => C::new(INF, -0.0),
            (PInf, PZero | Pos) => C::new(INF, 0.0),
            (PInf, PInf) => C::new(INF, FRAC_PI_4),
            (PInf, Nan) => C::new(INF, NAN),
            (Neg | NZero | PZero | Pos, NInf) => C::new(INF, -FRAC_PI_2),
            (Neg | NZero | PZero | Pos, PInf) => C::new(INF, FRAC_PI_2),
            (Nan, NInf | PInf) => C::new(INF, NAN),
            _ => C::new(NAN, NAN),
        };
        return Ok(res);
    }

    if z.re.abs() > CM_LARGE_DOUBLE || z.im.abs() > CM_LARGE_DOUBLE {
        let re = (z.re * 0.5).hypot(z.im * 0.5).ln() + 2.0 * LN_2;
        let im = z.im.atan2(z.re);
        Ok(C::new(re, im))
    } else {
        let s1 = c_sqrt(C::new(z.re - 1.0, z.im))?;
        let s2 = c_sqrt(C::new(z.re + 1.0, z.im))?;
        let re = (s1.re * s2.re + s1.im * s2.im).asinh();
        let im = 2.0 * s1.im.atan2(s2.re);
        Ok(C::new(re, im))
    }
}

/// Complex `asinh(z)` matching CPython 3.12's `c_asinh`.
pub(super) fn c_asinh(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        let (rx, ry) = (classify(z.re), classify(z.im));
        use SpecialType::*;
        let res = match (rx, ry) {
            (NInf, NInf) => C::new(NINF, -FRAC_PI_4),
            (NInf, Neg | NZero) => C::new(NINF, -0.0),
            (NInf, PZero | Pos) => C::new(NINF, 0.0),
            (NInf, PInf) => C::new(NINF, FRAC_PI_4),
            (NInf, Nan) => C::new(NINF, NAN),
            (PInf, NInf) => C::new(INF, -FRAC_PI_4),
            (PInf, Neg | NZero) => C::new(INF, -0.0),
            (PInf, PZero | Pos) => C::new(INF, 0.0),
            (PInf, PInf) => C::new(INF, FRAC_PI_4),
            (PInf, Nan) => C::new(INF, NAN),
            (Neg | NZero, NInf) => C::new(NINF, -FRAC_PI_2),
            (Neg | NZero, PInf) => C::new(NINF, FRAC_PI_2),
            (PZero | Pos, NInf) => C::new(INF, -FRAC_PI_2),
            (PZero | Pos, PInf) => C::new(INF, FRAC_PI_2),
            (Nan, NInf | PInf) => C::new(INF, NAN),
            (Nan, NZero) => C::new(NAN, -0.0),
            (Nan, PZero) => C::new(NAN, 0.0),
            _ => C::new(NAN, NAN),
        };
        return Ok(res);
    }

    if z.re.abs() > CM_LARGE_DOUBLE || z.im.abs() > CM_LARGE_DOUBLE {
        let mag = (z.re * 0.5).hypot(z.im * 0.5).ln() + 2.0 * LN_2;
        let re = mag.copysign(z.re);
        let im = z.im.atan2(z.re.abs());
        Ok(C::new(re, im))
    } else {
        let s1 = c_sqrt(C::new(1.0 + z.im, -z.re))?;
        let s2 = c_sqrt(C::new(1.0 - z.im, z.re))?;
        let re = (s1.re * s2.im - s2.re * s1.im).asinh();
        let im = z.im.atan2(s1.re * s2.re - s1.im * s2.im);
        Ok(C::new(re, im))
    }
}

/// Complex `atanh(z)` matching CPython 3.12's `c_atanh`.
pub(super) fn c_atanh(z: C) -> std::result::Result<C, MathErr> {
    if is_special(z) {
        let (rx, ry) = (classify(z.re), classify(z.im));
        use SpecialType::*;
        let res = match (rx, ry) {
            (NInf, NInf | Neg | NZero) => C::new(-0.0, -FRAC_PI_2),
            (NInf, PZero | Pos | PInf) => C::new(-0.0, FRAC_PI_2),
            (NInf, Nan) => C::new(-0.0, NAN),
            (PInf, NInf | Neg | NZero) => C::new(0.0, -FRAC_PI_2),
            (PInf, PZero | Pos | PInf) => C::new(0.0, FRAC_PI_2),
            (PInf, Nan) => C::new(0.0, NAN),
            (Neg | NZero, NInf) => C::new(-0.0, -FRAC_PI_2),
            (Neg | NZero, PInf) => C::new(-0.0, FRAC_PI_2),
            (PZero | Pos | Nan, NInf) => C::new(0.0, -FRAC_PI_2),
            (PZero | Pos | Nan, PInf) => C::new(0.0, FRAC_PI_2),
            (NZero, Nan) => C::new(-0.0, NAN),
            (PZero, Nan) => C::new(0.0, NAN),
            _ => C::new(NAN, NAN),
        };
        return Ok(res);
    }

    // Reduce to first quadrant using oddness and conjugation.
    let mut x = z.re;
    let mut y = z.im;
    let neg_x = x.is_sign_negative();
    let neg_y = y.is_sign_negative();
    if neg_x {
        x = -x;
    }
    if neg_y {
        y = -y;
    }

    let (re, im) = if x > CM_SQRT_LARGE_DOUBLE || y > CM_SQRT_LARGE_DOUBLE {
        // |z| is very large: atanh(z) ~ 1/z + i*pi/2
        let h = x.hypot(y);
        let re = (x / h) / h;
        (re, FRAC_PI_2)
    } else if x == 1.0 && y < CM_SQRT_DBL_MIN {
        if y == 0.0 {
            return Err(MathErr::Domain);
        }
        let re = -(y.sqrt().ln()) + (y.hypot(2.0).ln()) * 0.5;
        let im = 2.0_f64.atan2(-y) * 0.5;
        (re, im)
    } else {
        let ay = y;
        let re = (4.0 * x / ((1.0 - x) * (1.0 - x) + ay * ay)).ln_1p() * 0.25;
        let im = (2.0 * ay).atan2((1.0 - x) * (1.0 + x) - ay * ay) * 0.5;
        (re, im)
    };

    Ok(C::new(
        if neg_x { -re } else { re },
        if neg_y { -im } else { im },
    ))
}

/// Complex `asin(z) = -i * asinh(i * z)`.
pub(super) fn c_asin(z: C) -> std::result::Result<C, MathErr> {
    let s = c_asinh(C::new(-z.im, z.re))?;
    Ok(C::new(s.im, -s.re))
}

/// Complex `atan(z) = -i * atanh(i * z)`.
pub(super) fn c_atan(z: C) -> std::result::Result<C, MathErr> {
    let s = c_atanh(C::new(-z.im, z.re))?;
    Ok(C::new(s.im, -s.re))
}

/// Complex `sin(z) = -i * sinh(i * z)`.
pub(super) fn c_sin(z: C) -> std::result::Result<C, MathErr> {
    let s = c_sinh(C::new(-z.im, z.re))?;
    Ok(C::new(s.im, -s.re))
}

/// Complex `cos(z) = cosh(i * z)`.
pub(super) fn c_cos(z: C) -> std::result::Result<C, MathErr> {
    c_cosh(C::new(-z.im, z.re))
}

/// Complex `tan(z) = -i * tanh(i * z)`.
pub(super) fn c_tan(z: C) -> std::result::Result<C, MathErr> {
    let s = c_tanh(C::new(-z.im, z.re))?;
    Ok(C::new(s.im, -s.re))
}

pub(super) fn unary_complex(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
    op: fn(C) -> std::result::Result<C, MathErr>,
) -> Result<Value> {
    let (re, im) = single_complex(interp, fn_name, args)?;
    let out = op(C::new(re, im)).map_err(to_py_err)?;
    Ok(Value::complex(out.re, out.im))
}

pub(super) fn phase(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (re, im) = single_complex(interp, fn_name, args)?;
    Ok(Value::float(im.atan2(re)))
}

pub(super) fn polar(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (re, im) = single_complex(interp, fn_name, args)?;
    let r = re.hypot(im);
    if r.is_infinite() && re.is_finite() && im.is_finite() {
        return Err(overflow_err());
    }
    let phi = im.atan2(re);
    Ok(Value::tuple(vec![Value::float(r), Value::float(phi)]))
}

pub(super) fn rect(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (r, phi) = two_floats(interp, fn_name, args)?;
    // Special values matching CPython 3.12's `cmath_rect_impl`.
    if !r.is_finite() || !phi.is_finite() {
        // if r == 0 and phi is inf or nan -> 0j
        if r == 0.0 && (phi.is_infinite() || phi.is_nan()) {
            return Ok(Value::complex(0.0, 0.0));
        }
        // if phi is ±inf and r is finite nonzero or ±inf -> ValueError
        if phi.is_infinite() {
            if r.is_nan() {
                return Ok(Value::complex(NAN, NAN));
            }
            return Err(domain_err());
        }
        // if phi is NaN:
        if phi.is_nan() {
            if r.is_infinite() {
                return Ok(Value::complex(INF, NAN));
            }
            return Ok(Value::complex(NAN, NAN));
        }
        // phi is finite, r is ±inf or NaN:
        if r.is_infinite() {
            if phi == 0.0 {
                // r = ±inf, phi = ±0.0: cos(phi)=1, sin(phi)=phi
                let im = if r.is_sign_negative() { -phi } else { phi };
                return Ok(Value::complex(r, im));
            }
            let c = phi.cos();
            let s = phi.sin();
            return Ok(Value::complex(r * c, r * s));
        }
        // r is NaN, phi is finite:
        if phi == 0.0 {
            return Ok(Value::complex(NAN, 0.0));
        }
        return Ok(Value::complex(NAN, NAN));
    }
    let re = r * phi.cos();
    let im = r * phi.sin();
    Ok(Value::complex(re, im))
}

pub(super) fn log(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes one or two arguments"),
        ));
    }
    let (re, im) = cmath_arg_to_complex(interp, &args[0].value)?;
    let num = c_log(C::new(re, im)).map_err(to_py_err)?;
    if args.len() == 2 {
        let (bre, bim) = cmath_arg_to_complex(interp, &args[1].value)?;
        let den = c_log(C::new(bre, bim)).map_err(to_py_err)?;
        let q = c_quot(num, den).map_err(to_py_err)?;
        Ok(Value::complex(q.re, q.im))
    } else {
        Ok(Value::complex(num.re, num.im))
    }
}

pub(super) fn isfinite(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (re, im) = single_complex(interp, fn_name, args)?;
    Ok(Value::bool_(re.is_finite() && im.is_finite()))
}

pub(super) fn isinf(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (re, im) = single_complex(interp, fn_name, args)?;
    Ok(Value::bool_(re.is_infinite() || im.is_infinite()))
}

pub(super) fn isnan(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (re, im) = single_complex(interp, fn_name, args)?;
    Ok(Value::bool_(re.is_nan() || im.is_nan()))
}

pub(super) fn isclose(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let mut positional: Vec<&Value> = Vec::new();
    let mut rel_tol: f64 = 1e-9;
    let mut abs_tol: f64 = 0.0;
    let mut seen_rel = false;
    let mut seen_abs = false;

    for arg in args {
        match arg.name.as_deref() {
            None => positional.push(&arg.value),
            Some("rel_tol") => {
                if seen_rel {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for keyword argument 'rel_tol'"),
                    ));
                }
                seen_rel = true;
                rel_tol = cmath_arg_to_float(interp, &arg.value)?;
            }
            Some("abs_tol") => {
                if seen_abs {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for keyword argument 'abs_tol'"),
                    ));
                }
                seen_abs = true;
                abs_tol = cmath_arg_to_float(interp, &arg.value)?;
            }
            Some(kw) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{kw}' is an invalid keyword argument for isclose()"),
                ));
            }
        }
    }

    if positional.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "isclose() takes exactly 2 positional arguments ({} given)",
                positional.len()
            ),
        ));
    }

    let (ar, ai) = cmath_arg_to_complex(interp, positional[0])?;
    let (br, bi) = cmath_arg_to_complex(interp, positional[1])?;

    if rel_tol < 0.0 || abs_tol < 0.0 {
        return Err(PyError::named(
            "ValueError",
            "tolerances must be non-negative".to_string(),
        ));
    }

    if ar == br && ai == bi {
        return Ok(Value::bool_(true));
    }
    if ar.is_infinite() || ai.is_infinite() || br.is_infinite() || bi.is_infinite() {
        return Ok(Value::bool_(false));
    }

    let diff = (ar - br).hypot(ai - bi);
    let abs_a = ar.hypot(ai);
    let abs_b = br.hypot(bi);
    let close = diff <= (rel_tol * abs_a.max(abs_b)).max(abs_tol);
    Ok(Value::bool_(close))
}
