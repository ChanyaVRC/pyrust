// `colorsys` — 100% native Rust implementation of CPython 3.12's `colorsys`
// module (issues #3230, #3329).
//
// CPython's `Lib/colorsys.py` is pure Python providing conversions between
// RGB and the YIQ, HLS, and HSV color systems.  This file ports it line for
// line, with no Python-source injection.
//
// The Python source is written against *duck-typed numbers*: every argument
// flows through `*`, `+`, `-`, `/`, `%`, `<`, `==`, `max`, `min` and `int`, so
// ints, bools, floats, `Fraction`s and user types all work, and the result
// types follow Python's promotion rules (e.g. `rgb_to_hsv(1, 1, 1)` returns
// `(0.0, 0.0, 1)` — the *int* `v`).  To keep that exact behaviour the
// arithmetic goes through the interpreter's binary-operator dispatch; the
// common `float`/`float` case takes an IEEE 754 fast path that is bit-for-bit
// identical to CPython's `float_add` / `float_sub` / `float_mul` /
// `float_div` / `float_rem`.
//
// Reference: <https://docs.python.org/3/library/colorsys.html>

use crate::ast::BinaryOp;
use crate::error::Result;
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

const ONE_THIRD: f64 = 1.0 / 3.0;
const ONE_SIXTH: f64 = 1.0 / 6.0;
const TWO_THIRD: f64 = 2.0 / 3.0;

const COLORSYS_DOC: &str = "Conversion functions between RGB and other color systems.

This modules provides two functions for each color system ABC:

  rgb_to_abc(r, g, b) --> a, b, c
  abc_to_rgb(a, b, c) --> r, g, b

All inputs and outputs are triples of floats in the range [0.0...1.0]
(with the exception of I and Q, which covers a slightly larger range).
Inputs outside the valid range may cause exceptions or invalid outputs.

Supported color systems:
RGB: Red, Green, Blue components
YIQ: Luminance, Chrominance (used by composite video signals)
HLS: Hue, Luminance, Saturation
HSV: Hue, Saturation, Value
";

fn flt(x: f64) -> Value {
    Value::float(x)
}

fn as_f64(v: &Value) -> Option<f64> {
    match v.kind() {
        ValueKind::Float(f) => Some(f),
        _ => None,
    }
}

/// Python `float % float` for a non-zero divisor (`float_rem`).
fn float_rem(a: f64, b: f64) -> f64 {
    let mut m = a % b;
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m += b;
        }
    } else {
        m = 0.0_f64.copysign(b);
    }
    m
}

/// `a <op> b` with Python semantics.
fn arith(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<Value> {
    if let (Some(x), Some(y)) = (as_f64(a), as_f64(b)) {
        match op {
            BinaryOp::Add => return Ok(flt(x + y)),
            BinaryOp::Sub => return Ok(flt(x - y)),
            BinaryOp::Mul => return Ok(flt(x * y)),
            BinaryOp::Div if y != 0.0 => return Ok(flt(x / y)),
            BinaryOp::Mod if y != 0.0 => return Ok(flt(float_rem(x, y))),
            _ => {}
        }
    }
    interp.eval_binary(a.clone(), op, b.clone())
}

/// `a <cmp> b` truth value with Python semantics.
fn cmp(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<bool> {
    if let (Some(x), Some(y)) = (as_f64(a), as_f64(b)) {
        match op {
            BinaryOp::Lt => return Ok(x < y),
            BinaryOp::Le => return Ok(x <= y),
            BinaryOp::Gt => return Ok(x > y),
            BinaryOp::Eq => return Ok(x == y),
            _ => {}
        }
    }
    let res = interp.eval_binary(a.clone(), op, b.clone())?;
    interp.truthy_value(&res)
}

fn add(i: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    arith(i, a, BinaryOp::Add, b)
}
fn sub(i: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    arith(i, a, BinaryOp::Sub, b)
}
fn mul(i: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    arith(i, a, BinaryOp::Mul, b)
}
fn div(i: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    arith(i, a, BinaryOp::Div, b)
}
fn rem(i: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    arith(i, a, BinaryOp::Mod, b)
}

/// `max(a, b, c)` — first maximal element, replaced only on strict `>`.
fn max3(i: &mut Interpreter, a: &Value, b: &Value, c: &Value) -> Result<Value> {
    let mut best = a.clone();
    for item in [b, c] {
        if cmp(i, item, BinaryOp::Gt, &best)? {
            best = item.clone();
        }
    }
    Ok(best)
}

/// `min(a, b, c)` — first minimal element, replaced only on strict `<`.
fn min3(i: &mut Interpreter, a: &Value, b: &Value, c: &Value) -> Result<Value> {
    let mut best = a.clone();
    for item in [b, c] {
        if cmp(i, item, BinaryOp::Lt, &best)? {
            best = item.clone();
        }
    }
    Ok(best)
}

fn tuple3(a: Value, b: Value, c: Value) -> Value {
    Value::tuple(vec![a, b, c])
}

/// `if x < 0.0: x = 0.0` for one channel.
fn clamp_low(i: &mut Interpreter, x: Value) -> Result<Value> {
    Ok(if cmp(i, &x, BinaryOp::Lt, &flt(0.0))? {
        flt(0.0)
    } else {
        x
    })
}

/// `if x > 1.0: x = 1.0` for one channel.
fn clamp_high(i: &mut Interpreter, x: Value) -> Result<Value> {
    Ok(if cmp(i, &x, BinaryOp::Gt, &flt(1.0))? {
        flt(1.0)
    } else {
        x
    })
}

fn rgb_to_yiq(i: &mut Interpreter, r: &Value, g: &Value, b: &Value) -> Result<Value> {
    // y = 0.30 * r + 0.59 * g + 0.11 * b
    let t = mul(i, &flt(0.30), r)?;
    let u = mul(i, &flt(0.59), g)?;
    let t = add(i, &t, &u)?;
    let w = mul(i, &flt(0.11), b)?;
    let y = add(i, &t, &w)?;
    // i = 0.74 * (r - y) - 0.27 * (b - y)
    let ry = sub(i, r, &y)?;
    let t = mul(i, &flt(0.74), &ry)?;
    let by = sub(i, b, &y)?;
    let u = mul(i, &flt(0.27), &by)?;
    let ii = sub(i, &t, &u)?;
    // q = 0.48 * (r - y) + 0.41 * (b - y)
    let ry = sub(i, r, &y)?;
    let t = mul(i, &flt(0.48), &ry)?;
    let by = sub(i, b, &y)?;
    let u = mul(i, &flt(0.41), &by)?;
    let q = add(i, &t, &u)?;
    Ok(tuple3(y, ii, q))
}

fn yiq_to_rgb(i: &mut Interpreter, y: &Value, ii: &Value, q: &Value) -> Result<Value> {
    // r = y + 0.9468822170900693 * i + 0.6235565819861433 * q
    let t = mul(i, &flt(0.9468822170900693), ii)?;
    let t = add(i, y, &t)?;
    let u = mul(i, &flt(0.6235565819861433), q)?;
    let r = add(i, &t, &u)?;
    // g = y - 0.27478764629897834 * i - 0.6356910791873801 * q
    let t = mul(i, &flt(0.27478764629897834), ii)?;
    let t = sub(i, y, &t)?;
    let u = mul(i, &flt(0.6356910791873801), q)?;
    let g = sub(i, &t, &u)?;
    // b = y - 1.1085450346420322 * i + 1.7090069284064666 * q
    let t = mul(i, &flt(1.1085450346420322), ii)?;
    let t = sub(i, y, &t)?;
    let u = mul(i, &flt(1.7090069284064666), q)?;
    let b = add(i, &t, &u)?;
    // CPython clamps all three channels low (r, g, b), then all three high.
    let r = clamp_low(i, r)?;
    let g = clamp_low(i, g)?;
    let b = clamp_low(i, b)?;
    let r = clamp_high(i, r)?;
    let g = clamp_high(i, g)?;
    let b = clamp_high(i, b)?;
    Ok(tuple3(r, g, b))
}

/// Shared hue computation of `rgb_to_hls` / `rgb_to_hsv`.
fn hue(
    i: &mut Interpreter,
    r: &Value,
    g: &Value,
    b: &Value,
    maxc: &Value,
    rangec: &Value,
) -> Result<Value> {
    let rc = sub(i, maxc, r)?;
    let rc = div(i, &rc, rangec)?;
    let gc = sub(i, maxc, g)?;
    let gc = div(i, &gc, rangec)?;
    let bc = sub(i, maxc, b)?;
    let bc = div(i, &bc, rangec)?;
    let h = if cmp(i, r, BinaryOp::Eq, maxc)? {
        sub(i, &bc, &gc)?
    } else if cmp(i, g, BinaryOp::Eq, maxc)? {
        let t = add(i, &flt(2.0), &rc)?;
        sub(i, &t, &bc)?
    } else {
        let t = add(i, &flt(4.0), &gc)?;
        sub(i, &t, &rc)?
    };
    let h = div(i, &h, &flt(6.0))?;
    rem(i, &h, &flt(1.0))
}

fn rgb_to_hls(i: &mut Interpreter, r: &Value, g: &Value, b: &Value) -> Result<Value> {
    let maxc = max3(i, r, g, b)?;
    let minc = min3(i, r, g, b)?;
    let sumc = add(i, &maxc, &minc)?;
    let rangec = sub(i, &maxc, &minc)?;
    let l = div(i, &sumc, &flt(2.0))?;
    if cmp(i, &minc, BinaryOp::Eq, &maxc)? {
        return Ok(tuple3(flt(0.0), l, flt(0.0)));
    }
    let s = if cmp(i, &l, BinaryOp::Le, &flt(0.5))? {
        div(i, &rangec, &sumc)?
    } else {
        // Not always `2.0 - sumc`: gh-106498.
        let t = sub(i, &flt(2.0), &maxc)?;
        let t = sub(i, &t, &minc)?;
        div(i, &rangec, &t)?
    };
    let h = hue(i, r, g, b, &maxc, &rangec)?;
    Ok(tuple3(h, l, s))
}

fn hls_v(i: &mut Interpreter, m1: &Value, m2: &Value, h: &Value) -> Result<Value> {
    let h = rem(i, h, &flt(1.0))?;
    if cmp(i, &h, BinaryOp::Lt, &flt(ONE_SIXTH))? {
        // m1 + (m2 - m1) * hue * 6.0
        let d = sub(i, m2, m1)?;
        let t = mul(i, &d, &h)?;
        let t = mul(i, &t, &flt(6.0))?;
        return add(i, m1, &t);
    }
    if cmp(i, &h, BinaryOp::Lt, &flt(0.5))? {
        return Ok(m2.clone());
    }
    if cmp(i, &h, BinaryOp::Lt, &flt(TWO_THIRD))? {
        // m1 + (m2 - m1) * (TWO_THIRD - hue) * 6.0
        let d = sub(i, m2, m1)?;
        let e = sub(i, &flt(TWO_THIRD), &h)?;
        let t = mul(i, &d, &e)?;
        let t = mul(i, &t, &flt(6.0))?;
        return add(i, m1, &t);
    }
    Ok(m1.clone())
}

fn hls_to_rgb(i: &mut Interpreter, h: &Value, l: &Value, s: &Value) -> Result<Value> {
    if cmp(i, s, BinaryOp::Eq, &flt(0.0))? {
        return Ok(tuple3(l.clone(), l.clone(), l.clone()));
    }
    let m2 = if cmp(i, l, BinaryOp::Le, &flt(0.5))? {
        // l * (1.0 + s)
        let t = add(i, &flt(1.0), s)?;
        mul(i, l, &t)?
    } else {
        // l + s - (l * s)
        let t = add(i, l, s)?;
        let u = mul(i, l, s)?;
        sub(i, &t, &u)?
    };
    // m1 = 2.0 * l - m2
    let t = mul(i, &flt(2.0), l)?;
    let m1 = sub(i, &t, &m2)?;
    let h_plus = add(i, h, &flt(ONE_THIRD))?;
    let r = hls_v(i, &m1, &m2, &h_plus)?;
    let g = hls_v(i, &m1, &m2, h)?;
    let h_minus = sub(i, h, &flt(ONE_THIRD))?;
    let b = hls_v(i, &m1, &m2, &h_minus)?;
    Ok(tuple3(r, g, b))
}

fn rgb_to_hsv(i: &mut Interpreter, r: &Value, g: &Value, b: &Value) -> Result<Value> {
    let maxc = max3(i, r, g, b)?;
    let minc = min3(i, r, g, b)?;
    let rangec = sub(i, &maxc, &minc)?;
    if cmp(i, &minc, BinaryOp::Eq, &maxc)? {
        return Ok(tuple3(flt(0.0), flt(0.0), maxc));
    }
    let s = div(i, &rangec, &maxc)?;
    let h = hue(i, r, g, b, &maxc, &rangec)?;
    Ok(tuple3(h, s, maxc))
}

fn hsv_to_rgb(i: &mut Interpreter, h: &Value, s: &Value, v: &Value) -> Result<Value> {
    if cmp(i, s, BinaryOp::Eq, &flt(0.0))? {
        return Ok(tuple3(v.clone(), v.clone(), v.clone()));
    }
    let h6 = mul(i, h, &flt(6.0))?;
    let sector = i.call_function_expanded(
        Value::builtin_function("int"),
        &[ExpandedCallArg {
            name: None,
            value: h6.clone(),
        }],
    )?;
    let f = sub(i, &h6, &sector)?;
    // p = v * (1.0 - s)
    let one_s = sub(i, &flt(1.0), s)?;
    let p = mul(i, v, &one_s)?;
    // q = v * (1.0 - s * f)
    let sf = mul(i, s, &f)?;
    let t = sub(i, &flt(1.0), &sf)?;
    let q = mul(i, v, &t)?;
    // t = v * (1.0 - s * (1.0 - f))
    let one_f = sub(i, &flt(1.0), &f)?;
    let s_one_f = mul(i, s, &one_f)?;
    let t = sub(i, &flt(1.0), &s_one_f)?;
    let t = mul(i, v, &t)?;
    let sector = rem(i, &sector, &Value::int(6))?;
    Ok(match sector.as_int() {
        Some(0) => tuple3(v.clone(), t, p),
        Some(1) => tuple3(q, v.clone(), p),
        Some(2) => tuple3(p, v.clone(), t),
        Some(3) => tuple3(p, q, v.clone()),
        Some(4) => tuple3(t, p, v.clone()),
        Some(5) => tuple3(v.clone(), p, q),
        _ => Value::none(),
    })
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(COLORSYS_DOC),
        "__all__" => Value::list(
            [
                "rgb_to_yiq",
                "yiq_to_rgb",
                "rgb_to_hls",
                "hls_to_rgb",
                "rgb_to_hsv",
                "hsv_to_rgb",
            ]
            .iter()
            .map(|s| Value::string(*s))
            .collect(),
        ),
        "ONE_THIRD" => Value::float(ONE_THIRD),
        "ONE_SIXTH" => Value::float(ONE_SIXTH),
        "TWO_THIRD" => Value::float(TWO_THIRD),
    }

    fn rgb_to_yiq(r: PyValue, g: PyValue, b: PyValue) -> Result<Value> {
        rgb_to_yiq(_interp, &r.0, &g.0, &b.0)
    }

    fn yiq_to_rgb(y: PyValue, i: PyValue, q: PyValue) -> Result<Value> {
        yiq_to_rgb(_interp, &y.0, &i.0, &q.0)
    }

    fn rgb_to_hls(r: PyValue, g: PyValue, b: PyValue) -> Result<Value> {
        rgb_to_hls(_interp, &r.0, &g.0, &b.0)
    }

    fn hls_to_rgb(h: PyValue, l: PyValue, s: PyValue) -> Result<Value> {
        hls_to_rgb(_interp, &h.0, &l.0, &s.0)
    }

    fn _v(m1: PyValue, m2: PyValue, hue: PyValue) -> Result<Value> {
        hls_v(_interp, &m1.0, &m2.0, &hue.0)
    }

    fn rgb_to_hsv(r: PyValue, g: PyValue, b: PyValue) -> Result<Value> {
        rgb_to_hsv(_interp, &r.0, &g.0, &b.0)
    }

    fn hsv_to_rgb(h: PyValue, s: PyValue, v: PyValue) -> Result<Value> {
        hsv_to_rgb(_interp, &h.0, &s.0, &v.0)
    }
}
