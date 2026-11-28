//! The public functions of CPython 3.12.3's `statistics`, ported statement by
//! statement.  Generic arithmetic goes through the interpreter's operators so
//! `int`, `float`, `Fraction`, `Decimal` and user numeric types behave as in
//! CPython; the exact sums come from `exact.rs`.

use num_bigint::BigInt;

use super::exact::{Num, Ratio, SumAcc, convert, num_div, sqrt_of_mss, ss, sum};
use super::ops::{
    binop, builtin, call, call_method, compare, int_value, is, items, length,
    linear_regression_class, math_call, module_attr, repr, stats_error, to_bigint, to_f64, truthy,
    try_swallow, type_error, value_error,
};
use crate::ast::BinaryOp;
use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::Value;

// ── Averages ──────────────────────────────────────────────────────────────────

pub(super) fn mean(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let (t, total, n) = sum(interp, data)?;
    if n < 1 {
        return Err(stats_error("mean requires at least one data point"));
    }
    let quotient = total.div_int(interp, n)?;
    convert(interp, quotient, &t)
}

pub(super) fn fmean(
    interp: &mut Interpreter,
    data: &Value,
    weights: Option<&Value>,
) -> Result<Value> {
    let Some(weights) = weights else {
        // try: n = len(data) except TypeError: count while iterating.
        let sized = try_swallow(
            || length(interp, data),
            |err| err.class_name_is("TypeError"),
        )?;
        let (n, source) = match sized {
            Ok(n) => (n, data.clone()),
            Err(_) => {
                let values = items(interp, data)?;
                (values.len(), Value::list(values))
            }
        };
        let total = math_call(interp, "fsum", &[source])?;
        if n == 0 {
            return Err(stats_error("fmean requires at least one data point"));
        }
        return binop(interp, &total, BinaryOp::Div, &Value::int(n as i64));
    };
    let weights = if weights.as_list().is_some() || weights.as_tuple().is_some() {
        weights.clone()
    } else {
        Value::list(items(interp, weights)?)
    };
    let num = try_swallow(
        || math_call(interp, "sumprod", &[data.clone(), weights.clone()]),
        |err| err.class_name_is("ValueError"),
    )?;
    let num = match num {
        Ok(num) => num,
        Err(_) => {
            return Err(stats_error("data and weights must be the same length"));
        }
    };
    let den = math_call(interp, "fsum", std::slice::from_ref(&weights))?;
    if !truthy(interp, &den)? {
        return Err(stats_error("sum of weights must be non-zero"));
    }
    binop(interp, &num, BinaryOp::Div, &den)
}

pub(super) fn geometric_mean(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let attempt = try_swallow(
        || {
            let log = module_attr(interp, "math", "log")?;
            let mapped = builtin(interp, "map", &[log, data.clone()])?;
            let mean = fmean(interp, &mapped, None)?;
            math_call(interp, "exp", &[mean])
        },
        |err| err.class_name_is("ValueError"),
    )?;
    match attempt {
        Ok(value) => Ok(value),
        Err(_) => Err(stats_error(
            "geometric mean requires a non-empty dataset containing positive numbers",
        )),
    }
}

const NEGATIVE_ERRMSG: &str = "harmonic mean does not support negative values";

pub(super) fn harmonic_mean(
    interp: &mut Interpreter,
    data: &Value,
    weights: Option<&Value>,
) -> Result<Value> {
    let mut data = data.clone();
    let iterator = builtin(interp, "iter", std::slice::from_ref(&data))?;
    if is(&iterator, &data) {
        data = Value::list(items(interp, &data)?);
    }
    let n = length(interp, &data)?;
    if n < 1 {
        return Err(stats_error(
            "harmonic_mean requires at least one data point",
        ));
    } else if n == 1 && weights.is_none() {
        let x = interp.eval_index(&data, Value::int(0))?;
        let real = module_attr(interp, "numbers", "Real")?;
        let decimal = module_attr(interp, "decimal", "Decimal")?;
        let kinds = Value::tuple(vec![real, decimal]);
        let ok = builtin(interp, "isinstance", &[x.clone(), kinds])?;
        if truthy(interp, &ok)? {
            if compare(interp, &x, BinaryOp::Lt, &Value::int(0))? {
                return Err(stats_error(NEGATIVE_ERRMSG));
            }
            return Ok(x);
        }
        return Err(type_error("unsupported type"));
    }
    let (weight_values, sum_weights) = match weights {
        None => (vec![Value::int(1); n], None),
        Some(weights) => {
            let mut weights = weights.clone();
            let iterator = builtin(interp, "iter", std::slice::from_ref(&weights))?;
            if is(&iterator, &weights) {
                weights = Value::list(items(interp, &weights)?);
            }
            if length(interp, &weights)? != n {
                return Err(stats_error("Number of weights does not match data size"));
            }
            let mut acc = SumAcc::new();
            for w in items(interp, &weights)? {
                if compare(interp, &w, BinaryOp::Lt, &Value::int(0))? {
                    return Err(stats_error(NEGATIVE_ERRMSG));
                }
                acc.add(interp, &w)?;
            }
            let (_, sum_weights, _) = acc.finish(interp)?;
            (items(interp, &weights)?, Some(sum_weights))
        }
    };
    let attempt = try_swallow(
        || {
            let data_values = items(interp, &data)?;
            let mut acc = SumAcc::new();
            for (w, x) in weight_values.iter().zip(data_values.iter()) {
                if compare(interp, x, BinaryOp::Lt, &Value::int(0))? {
                    return Err(stats_error(NEGATIVE_ERRMSG));
                }
                let term = if truthy(interp, w)? {
                    binop(interp, w, BinaryOp::Div, x)?
                } else {
                    Value::int(0)
                };
                acc.add(interp, &term)?;
            }
            acc.finish(interp)
        },
        |err| err.class_name_is("ZeroDivisionError"),
    )?;
    let Ok((t, total, _count)) = attempt else {
        return Ok(Value::int(0));
    };
    if total.is_non_positive(interp)? {
        return Err(stats_error("Weighted sum must be positive"));
    }
    // `sum_weights` is the int `n` without weights: exact against a rational
    // total, the plain int against a NaN / infinity.
    let sum_weights = match (sum_weights, &total) {
        (Some(sum_weights), _) => sum_weights,
        (None, Num::Exact(_)) => Num::Exact(Ratio::from_int(n)),
        (None, Num::Py(_)) => Num::Py(Value::int(n as i64)),
    };
    let quotient = num_div(interp, &sum_weights, &total)?;
    convert(interp, quotient, &t)
}

// ── Medians ───────────────────────────────────────────────────────────────────

/// `sorted(data)`.
fn sorted(interp: &mut Interpreter, data: &Value) -> Result<Vec<Value>> {
    let list = builtin(interp, "sorted", std::slice::from_ref(data))?;
    Ok(list.as_list().map(|l| l.clone()).unwrap_or_default())
}

pub(super) fn median(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let data = sorted(interp, data)?;
    let n = data.len();
    if n == 0 {
        return Err(stats_error("no median for empty data"));
    }
    if n % 2 == 1 {
        Ok(data[n / 2].clone())
    } else {
        let i = n / 2;
        let total = binop(interp, &data[i - 1], BinaryOp::Add, &data[i])?;
        binop(interp, &total, BinaryOp::Div, &Value::int(2))
    }
}

pub(super) fn median_low(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let data = sorted(interp, data)?;
    let n = data.len();
    if n == 0 {
        return Err(stats_error("no median for empty data"));
    }
    if n % 2 == 1 {
        Ok(data[n / 2].clone())
    } else {
        Ok(data[n / 2 - 1].clone())
    }
}

pub(super) fn median_high(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let data = sorted(interp, data)?;
    let n = data.len();
    if n == 0 {
        return Err(stats_error("no median for empty data"));
    }
    Ok(data[n / 2].clone())
}

pub(super) fn median_grouped(
    interp: &mut Interpreter,
    data: &Value,
    interval: &Value,
) -> Result<Value> {
    let data = sorted(interp, data)?;
    let n = data.len();
    if n == 0 {
        return Err(stats_error("no median for empty data"));
    }
    // Find the value at the midpoint.
    let x = data[n / 2].clone();

    // `bisect_left(data, x)`: first index with `not data[i] < x`.
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if compare(interp, &data[mid], BinaryOp::Lt, &x)? {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let i = lo;
    // `bisect_right(data, x, lo=i)`.
    let (mut lo, mut hi) = (i, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if compare(interp, &x, BinaryOp::Lt, &data[mid])? {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let j = lo;

    // Coerce to floats, raising a TypeError if not possible.
    let coerced = try_swallow(
        || {
            let interval = to_f64(interp, interval)?;
            let x = to_f64(interp, &x)?;
            Ok((interval, x))
        },
        |err| err.class_name_is("ValueError"),
    )?;
    let (interval, x) = match coerced {
        Ok(pair) => pair,
        Err(_) => return Err(type_error("Value cannot be converted to a float")),
    };
    let lower = x - interval / 2.0; // Lower limit of the median interval
    let cf = i as f64; // Cumulative frequency of the preceding interval
    let f = (j - i) as f64; // Number of elements in the median interval
    Ok(Value::float(lower + interval * (n as f64 / 2.0 - cf) / f))
}

// ── Modes ─────────────────────────────────────────────────────────────────────

/// `Counter(iter(data))`.
fn counter_of(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let iterator = builtin(interp, "iter", std::slice::from_ref(data))?;
    let counter = module_attr(interp, "collections", "Counter")?;
    call(interp, counter, &[iterator])
}

pub(super) fn mode(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let counts = counter_of(interp, data)?;
    let pairs = call_method(interp, &counts, "most_common", &[Value::int(1)])?;
    let pairs = items(interp, &pairs)?;
    match pairs.first() {
        Some(pair) => interp.eval_index(pair, Value::int(0)),
        None => Err(stats_error("no mode for empty data")),
    }
}

pub(super) fn multimode(interp: &mut Interpreter, data: &Value) -> Result<Value> {
    let counts = counter_of(interp, data)?;
    if !truthy(interp, &counts)? {
        return Ok(Value::list(Vec::new()));
    }
    let values = call_method(interp, &counts, "values", &[])?;
    let maxcount = builtin(interp, "max", &[values])?;
    let pairs = call_method(interp, &counts, "items", &[])?;
    let mut result = Vec::new();
    for pair in items(interp, &pairs)? {
        let (value, count) = match pair.as_tuple() {
            Some([value, count]) => (value.clone(), count.clone()),
            _ => continue,
        };
        if compare(interp, &count, BinaryOp::Eq, &maxcount)? {
            result.push(value);
        }
    }
    Ok(Value::list(result))
}

// ── Quantiles ─────────────────────────────────────────────────────────────────

pub(super) fn quantiles(
    interp: &mut Interpreter,
    data: &Value,
    n: &Value,
    method: &Value,
) -> Result<Value> {
    if compare(interp, n, BinaryOp::Lt, &Value::int(1))? {
        return Err(stats_error("n must be at least 1"));
    }
    let data = sorted(interp, data)?;
    let ld = data.len() as i64;
    if ld < 2 {
        return Err(stats_error("must have at least two data points"));
    }
    let inclusive = compare(interp, method, BinaryOp::Eq, &Value::string("inclusive"))?;
    let exclusive =
        !inclusive && compare(interp, method, BinaryOp::Eq, &Value::string("exclusive"))?;
    if !inclusive && !exclusive {
        let text = repr(interp, method)?;
        return Err(value_error(format!("Unknown method: {text}")));
    }
    // `range(1, n)`: raises `TypeError` for a non-integer `n`.
    let range = builtin(interp, "range", &[Value::int(1), n.clone()])?;
    let steps = items(interp, &range)?;
    let n_int: BigInt = match to_bigint(n) {
        Some(n) => n,
        None => {
            let index = builtin(interp, "int", std::slice::from_ref(n))?;
            to_bigint(&index).unwrap_or_default()
        }
    };
    let n_value = int_value(n_int.clone());
    let mut result = Vec::with_capacity(steps.len());
    for step in &steps {
        let i = to_bigint(step).unwrap_or_default();
        let interpolated = if inclusive {
            let m = BigInt::from(ld - 1);
            let (j, delta) = pyrust_core::bigint_divmod_floor(&(&i * &m), &n_int);
            let j_index = bigint_index(&j);
            let (low, high) = (data[j_index].clone(), data[j_index + 1].clone());
            let weight_low = int_value(&n_int - &delta);
            let weight_high = int_value(delta);
            let left = binop(interp, &low, BinaryOp::Mul, &weight_low)?;
            let right = binop(interp, &high, BinaryOp::Mul, &weight_high)?;
            let total = binop(interp, &left, BinaryOp::Add, &right)?;
            binop(interp, &total, BinaryOp::Div, &n_value)?
        } else {
            let m = BigInt::from(ld + 1);
            let scaled = &i * &m;
            let mut j = pyrust_core::bigint_divmod_floor(&scaled, &n_int).0; // rescale i to m/n
            let upper = BigInt::from(ld - 1);
            if j < BigInt::from(1) {
                j = BigInt::from(1);
            } else if j > upper {
                j = upper;
            }
            let delta = &scaled - &j * &n_int; // exact integer math
            let j_index = bigint_index(&j);
            let (low, high) = (data[j_index - 1].clone(), data[j_index].clone());
            let weight_low = int_value(&n_int - &delta);
            let weight_high = int_value(delta);
            let left = binop(interp, &low, BinaryOp::Mul, &weight_low)?;
            let right = binop(interp, &high, BinaryOp::Mul, &weight_high)?;
            let total = binop(interp, &left, BinaryOp::Add, &right)?;
            binop(interp, &total, BinaryOp::Div, &n_value)?
        };
        result.push(interpolated);
    }
    Ok(Value::list(result))
}

fn bigint_index(n: &BigInt) -> usize {
    use num_traits::ToPrimitive;
    n.to_usize().unwrap_or(usize::MAX)
}

// ── Measures of spread ────────────────────────────────────────────────────────

fn optional(value: &Option<Value>) -> Option<&Value> {
    value.as_ref().filter(|v| !v.is_none())
}

pub(super) fn variance(
    interp: &mut Interpreter,
    data: &Value,
    xbar: &Option<Value>,
) -> Result<Value> {
    let result = ss(interp, data, optional(xbar))?;
    if result.count < 2 {
        return Err(stats_error("variance requires at least two data points"));
    }
    let quotient = result.ssd.div_int(interp, result.count - 1)?;
    convert(interp, quotient, &result.t)
}

pub(super) fn pvariance(
    interp: &mut Interpreter,
    data: &Value,
    mu: &Option<Value>,
) -> Result<Value> {
    let result = ss(interp, data, optional(mu))?;
    if result.count < 1 {
        return Err(stats_error("pvariance requires at least one data point"));
    }
    let quotient = result.ssd.div_int(interp, result.count)?;
    convert(interp, quotient, &result.t)
}

pub(super) fn stdev(interp: &mut Interpreter, data: &Value, xbar: &Option<Value>) -> Result<Value> {
    let result = ss(interp, data, optional(xbar))?;
    if result.count < 2 {
        return Err(stats_error("stdev requires at least two data points"));
    }
    let mss = result.ssd.div_int(interp, result.count - 1)?;
    sqrt_of_mss(interp, &result.t, &mss)
}

pub(super) fn pstdev(interp: &mut Interpreter, data: &Value, mu: &Option<Value>) -> Result<Value> {
    let result = ss(interp, data, optional(mu))?;
    if result.count < 1 {
        return Err(stats_error("pstdev requires at least one data point"));
    }
    let mss = result.ssd.div_int(interp, result.count)?;
    sqrt_of_mss(interp, &result.t, &mss)
}

/// `_mean_stdev(data)`: the mean and sample standard deviation as floats.
pub(super) fn mean_stdev(interp: &mut Interpreter, data: &Value) -> Result<(Value, Value)> {
    let result = ss(interp, data, None)?;
    if result.count < 2 {
        return Err(stats_error("stdev requires at least two data points"));
    }
    let mss = result.ssd.div_int(interp, result.count - 1)?;
    let attempt = try_swallow(
        || {
            let (n, m) = super::exact::numerator_denominator(interp, &mss)?;
            let xbar = float_of_num(interp, &result.c)?;
            let stdev = super::exact::float_sqrt_of_frac(interp, &n, &m)?;
            Ok((xbar, stdev))
        },
        |err| err.class_name_is("AttributeError"),
    )?;
    match attempt {
        Ok(pair) => Ok(pair),
        Err(_) => {
            // Handle NaNs and infinities gracefully.
            let xbar = float_of_num(interp, &result.c)?;
            let ssd = float_of_num(interp, &result.ssd)?;
            let stdev = binop(interp, &xbar, BinaryOp::Div, &ssd)?;
            Ok((xbar, stdev))
        }
    }
}

/// `float(num)`.
fn float_of_num(interp: &mut Interpreter, num: &Num) -> Result<Value> {
    match num {
        Num::Exact(r) => binop(
            interp,
            &int_value(r.num.clone()),
            BinaryOp::Div,
            &int_value(r.den.clone()),
        ),
        Num::Py(value) => Ok(Value::float(to_f64(interp, value)?)),
    }
}

// ── Relations between two inputs ──────────────────────────────────────────────

/// `fsum(values) / n`.
fn mean_of(interp: &mut Interpreter, values: &Value, n: usize) -> Result<Value> {
    let total = math_call(interp, "fsum", std::slice::from_ref(values))?;
    binop(interp, &total, BinaryOp::Div, &Value::int(n as i64))
}

/// `[v - center for v in values]`.
fn centered(interp: &mut Interpreter, values: &Value, center: &Value) -> Result<Value> {
    let mut result = Vec::new();
    for v in items(interp, values)? {
        result.push(binop(interp, &v, BinaryOp::Sub, center)?);
    }
    Ok(Value::list(result))
}

fn sumprod(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    math_call(interp, "sumprod", &[a.clone(), b.clone()])
}

/// The `len(x)` / `len(y)` / `n < 2` checks the two-input functions share.
fn two_input_size(
    interp: &mut Interpreter,
    x: &Value,
    y: &Value,
    mismatch: &str,
    too_few: &str,
) -> Result<usize> {
    let n = length(interp, x)?;
    if length(interp, y)? != n {
        return Err(stats_error(mismatch));
    }
    if n < 2 {
        return Err(stats_error(too_few));
    }
    Ok(n)
}

pub(super) fn covariance(interp: &mut Interpreter, x: &Value, y: &Value) -> Result<Value> {
    let n = two_input_size(
        interp,
        x,
        y,
        "covariance requires that both inputs have same number of data points",
        "covariance requires at least two data points",
    )?;
    let xbar = mean_of(interp, x, n)?;
    let ybar = mean_of(interp, y, n)?;
    let dx = centered(interp, x, &xbar)?;
    let dy = centered(interp, y, &ybar)?;
    let sxy = sumprod(interp, &dx, &dy)?;
    binop(interp, &sxy, BinaryOp::Div, &Value::int(n as i64 - 1))
}

/// `_rank(data, start=start)`: the average-tie ranks of `data`.
fn rank(interp: &mut Interpreter, data: &Value, start: f64) -> Result<Value> {
    let values = items(interp, data)?;
    let pairs: Vec<Value> = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| Value::tuple(vec![value, Value::int(index as i64)]))
        .collect();
    let ordered = builtin(interp, "sorted", &[Value::list(pairs)])?;
    let ordered: Vec<Value> = ordered.as_list().map(|l| l.clone()).unwrap_or_default();
    let mut result = vec![Value::int(0); ordered.len()];
    let mut i = start - 1.0;
    let mut position = 0usize;
    while position < ordered.len() {
        let key = first_of(&ordered[position]);
        let mut end = position + 1;
        while end < ordered.len() {
            let candidate = first_of(&ordered[end]);
            if !compare(interp, &candidate, BinaryOp::Eq, &key)? {
                break;
            }
            end += 1;
        }
        let size = end - position;
        let rank = i + (size as f64 + 1.0) / 2.0;
        for item in &ordered[position..end] {
            let original = item
                .as_tuple()
                .and_then(|t| t.get(1))
                .and_then(Value::as_int)
                .unwrap_or_default();
            result[original as usize] = Value::float(rank);
        }
        i += size as f64;
        position = end;
    }
    Ok(Value::list(result))
}

fn first_of(pair: &Value) -> Value {
    pair.as_tuple()
        .and_then(|t| t.first().cloned())
        .unwrap_or_else(Value::none)
}

pub(super) fn correlation(
    interp: &mut Interpreter,
    x: &Value,
    y: &Value,
    method: &Value,
) -> Result<Value> {
    let n = two_input_size(
        interp,
        x,
        y,
        "correlation requires that both inputs have same number of data points",
        "correlation requires at least two data points",
    )?;
    // `method not in {'linear', 'ranked'}`: hashes `method` first.
    let known = match method.as_str() {
        Some(text) => text == "linear" || text == "ranked",
        None => {
            builtin(interp, "hash", std::slice::from_ref(method))?;
            compare(interp, method, BinaryOp::Eq, &Value::string("linear"))?
                || compare(interp, method, BinaryOp::Eq, &Value::string("ranked"))?
        }
    };
    if !known {
        let text = repr(interp, method)?;
        return Err(value_error(format!("Unknown method: {text}")));
    }
    let ranked = compare(interp, method, BinaryOp::Eq, &Value::string("ranked"))?;
    let (x, y) = if ranked {
        let start = (n as f64 - 1.0) / -2.0; // Center rankings around zero
        (rank(interp, x, start)?, rank(interp, y, start)?)
    } else {
        let xbar = mean_of(interp, x, n)?;
        let ybar = mean_of(interp, y, n)?;
        (centered(interp, x, &xbar)?, centered(interp, y, &ybar)?)
    };
    let sxy = sumprod(interp, &x, &y)?;
    let sxx = sumprod(interp, &x, &x)?;
    let syy = sumprod(interp, &y, &y)?;
    let attempt = try_swallow(
        || {
            let product = binop(interp, &sxx, BinaryOp::Mul, &syy)?;
            let root = math_call(interp, "sqrt", &[product])?;
            binop(interp, &sxy, BinaryOp::Div, &root)
        },
        |err| err.class_name_is("ZeroDivisionError"),
    )?;
    match attempt {
        Ok(value) => Ok(value),
        Err(_) => Err(stats_error("at least one of the inputs is constant")),
    }
}

pub(super) fn linear_regression(
    interp: &mut Interpreter,
    x: &Value,
    y: &Value,
    proportional: bool,
) -> Result<Value> {
    let n = two_input_size(
        interp,
        x,
        y,
        "linear regression requires that both inputs have same number of data points",
        "linear regression requires at least two data points",
    )?;
    let (x, y, bars) = if !proportional {
        let xbar = mean_of(interp, x, n)?;
        let ybar = mean_of(interp, y, n)?;
        let dx = centered(interp, x, &xbar)?; // List because used three times below
        let dy = centered(interp, y, &ybar)?;
        (dx, dy, Some((xbar, ybar)))
    } else {
        (x.clone(), y.clone(), None)
    };
    let product = sumprod(interp, &x, &y)?;
    let sxy = binop(interp, &product, BinaryOp::Add, &Value::float(0.0))?; // coerce to a float
    let sxx = sumprod(interp, &x, &x)?;
    let attempt = try_swallow(
        || binop(interp, &sxy, BinaryOp::Div, &sxx),
        |err| err.class_name_is("ZeroDivisionError"),
    )?;
    let Ok(slope) = attempt else {
        return Err(stats_error("x is constant"));
    };
    let intercept = match bars {
        None => Value::float(0.0),
        Some((xbar, ybar)) => {
            let scaled = binop(interp, &slope, BinaryOp::Mul, &xbar)?;
            binop(interp, &ybar, BinaryOp::Sub, &scaled)?
        }
    };
    let class = linear_regression_class()?;
    call(interp, class, &[slope, intercept])
}
