//! Complex arithmetic algorithms mirroring CPython's `Objects/complexobject.c`.

use crate::errors::Result;
use crate::object_model::Value;

/// Complex division mirroring CPython's `_Py_c_quot` (Smith's scaling formula
/// in `Objects/complexobject.c`) so large/tiny magnitudes don't spuriously
/// overflow/underflow `br² + bi²`.
pub(super) fn complex_div(a: (f64, f64), b: (f64, f64)) -> Result<Value> {
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
pub(super) fn complex_pow(zr: f64, zi: f64, wr: f64, wi: f64) -> Result<Value> {
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
