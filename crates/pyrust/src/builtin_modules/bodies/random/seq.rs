//! The integer, bytes and sequence methods of CPython 3.12.3's
//! `random.Random`: `randbytes`, `randrange`, `randint`, `choice`, `shuffle`,
//! `sample` and `choices`.

use std::collections::HashSet;

use num_bigint::BigInt;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::ast::BinaryOp;
use crate::builtin_modules::_random::{self as core, u64_value, with_mt};
use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{Value, ValueKind};

use super::dispatch::{
    Below, BitsSrc, NativeId, RandomSrc, Resolved, below, bits_src, random_src, resolve,
};
use super::support::{
    bigint_value, binop, builtin, call, call_kw, call_method, compare, exact_int, float_operand,
    format_value, getitem, getitem_at, index, index_error, int_value_of, is_int, length, setitem,
    type_error, value_error,
};

/// `self.getrandbits(n * 8).to_bytes(n, 'little')`.
pub(super) fn randbytes(interp: &mut Interpreter, this: &Value, n: &Value) -> Result<Value> {
    let bits = bits_src(interp, this)?;
    if let (BitsSrc::Mt, Some(count)) = (&bits, exact_int(n)) {
        let k = core::getrandbits_arg(interp, &bigint_value(BigInt::from(count) * 8))?;
        let count = (k / 8) as usize;
        return with_mt(this, |mt| Value::bytes(mt.randbytes(count)));
    }
    let k = binop(interp, n, BinaryOp::Mul, &Value::int(8))?;
    let r = match &bits {
        BitsSrc::Py(f) => call(interp, f.clone(), &[k])?,
        native => {
            let k = match native {
                BitsSrc::Mt => core::getrandbits_arg(interp, &k)?,
                _ => sys_bits_arg(interp, &k)?,
            };
            native.bits(interp, this, k)?
        }
    };
    call_method(
        interp,
        &r,
        "to_bytes",
        &[n.clone(), Value::string("little")],
    )
}

/// `SystemRandom.getrandbits`'s `if k < 0` check for an `int` `k`.
fn sys_bits_arg(interp: &mut Interpreter, k: &Value) -> Result<u64> {
    let value = index(interp, k)?;
    if value.is_negative() {
        return Err(value_error("number of bits must be non-negative"));
    }
    value.to_u64().ok_or_else(|| {
        crate::error::PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

/// `step is _ONE`: the default, or an exact `int` 1 (CPython's small-int
/// cache makes every `1` literal the same object).
fn is_one(step: Option<&Value>) -> bool {
    step.is_none_or(|s| matches!(s.kind(), ValueKind::Int(1)))
}

/// `istart + istep * randbelow(n)`.
fn offset(interp: &mut Interpreter, start: &BigInt, step: &BigInt, drawn: Value) -> Result<Value> {
    if let Some(d) = int_value_of(&drawn)
        .filter(|_| matches!(drawn.kind(), ValueKind::Int(_) | ValueKind::BigInt(_)))
    {
        return Ok(bigint_value(start + step * d));
    }
    let scaled = if step.is_one() {
        drawn
    } else {
        binop(interp, &bigint_value(step.clone()), BinaryOp::Mul, &drawn)?
    };
    binop(interp, &bigint_value(start.clone()), BinaryOp::Add, &scaled)
}

/// `randbelow(n)` for a positive `int` `n`.
fn draw_below(interp: &mut Interpreter, this: &Value, below: &Below, n: &BigInt) -> Result<Value> {
    if below.is_native()
        && let Some(small) = n.to_u64()
        && (matches!(below, Below::With(_)) || small < super::dispatch::MAXSIZE)
    {
        return Ok(u64_value(below.below_native(this, small)?));
    }
    below.call(interp, this, &bigint_value(n.clone()))
}

/// `randrange(start, stop=None, step=1)`.
pub(super) fn randrange(
    interp: &mut Interpreter,
    this: &Value,
    start: &Value,
    stop: Option<&Value>,
    step: Option<&Value>,
) -> Result<Value> {
    let istart = index(interp, start)?;
    let Some(stop) = stop.filter(|s| !s.is_none()) else {
        if !is_one(step) {
            return Err(type_error("Missing a non-None stop argument"));
        }
        if istart.is_positive() {
            let randbelow = below(interp, this)?;
            return draw_below(interp, this, &randbelow, &istart);
        }
        return Err(value_error("empty range for randrange()"));
    };
    let istop = index(interp, stop)?;
    let width = &istop - &istart;
    let one = Value::int(1);
    let istep = index(interp, step.unwrap_or(&one))?;
    if istep.is_one() {
        if width.is_positive() {
            let randbelow = below(interp, this)?;
            let drawn = draw_below(interp, this, &randbelow, &width)?;
            return offset(interp, &istart, &BigInt::one(), drawn);
        }
        let (a, b) = (format_value(interp, start)?, format_value(interp, stop)?);
        return Err(value_error(format!("empty range in randrange({a}, {b})")));
    }
    let n = if istep.is_positive() {
        floor_div(&(&width + &istep - 1), &istep)
    } else if istep.is_negative() {
        floor_div(&(&width + &istep + 1), &istep)
    } else {
        return Err(value_error("zero step for randrange()"));
    };
    if !n.is_positive() {
        let step = step.cloned().unwrap_or(one);
        let (a, b, c) = (
            format_value(interp, start)?,
            format_value(interp, stop)?,
            format_value(interp, &step)?,
        );
        return Err(value_error(format!(
            "empty range in randrange({a}, {b}, {c})"
        )));
    }
    let randbelow = below(interp, this)?;
    let drawn = draw_below(interp, this, &randbelow, &n)?;
    offset(interp, &istart, &istep, drawn)
}

/// Python's `//` on integers.
fn floor_div(a: &BigInt, b: &BigInt) -> BigInt {
    let (q, r) = (a / b, a % b);
    if !r.is_zero() && r.is_negative() != b.is_negative() {
        q - 1
    } else {
        q
    }
}

/// `randint(a, b)`: `self.randrange(a, b+1)`.
pub(super) fn randint(
    interp: &mut Interpreter,
    this: &Value,
    a: &Value,
    b: &Value,
) -> Result<Value> {
    let randrange_fn = resolve(interp, this, "randrange", &[NativeId::Randrange])?;
    let stop = match exact_int(b).and_then(|b| b.checked_add(1)) {
        Some(stop) => Value::int(stop),
        None => binop(interp, b, BinaryOp::Add, &Value::int(1))?,
    };
    match randrange_fn {
        Resolved::Native(_) => randrange(interp, this, a, Some(&stop), None),
        Resolved::Bound(f) => call(interp, f, &[a.clone(), stop]),
    }
}

/// `choice(seq)`.
pub(super) fn choice(interp: &mut Interpreter, this: &Value, seq: &Value) -> Result<Value> {
    if length(interp, seq)? == 0 {
        return Err(index_error("Cannot choose from an empty sequence"));
    }
    let randbelow = below(interp, this)?;
    let n = length(interp, seq)?;
    if randbelow.is_native()
        && n > 0
        && (matches!(randbelow, Below::With(_)) || (n as u64) < super::dispatch::MAXSIZE)
    {
        let i = randbelow.below_native(this, n as u64)?;
        return getitem_at(interp, seq, i as usize);
    }
    let i = randbelow.call(interp, this, &Value::int(n as i64))?;
    getitem(interp, seq, i)
}

/// `shuffle(x)`.
pub(super) fn shuffle(interp: &mut Interpreter, this: &Value, x: &Value) -> Result<()> {
    let randbelow = below(interp, this)?;
    let n = length(interp, x)?;
    if randbelow.is_native() && matches!(x.kind(), ValueKind::List(_)) {
        for i in (1..n).rev() {
            let j = randbelow.below_native(this, i as u64 + 1)? as usize;
            let swapped = x.list_with_mut(|items| {
                if i < items.len() && j < items.len() {
                    items.swap(i, j);
                    true
                } else {
                    false
                }
            });
            if swapped != Some(true) {
                return Err(index_error("list index out of range"));
            }
        }
        return Ok(());
    }
    for i in (1..n).rev() {
        let j = randbelow.call(interp, this, &Value::int(i as i64 + 1))?;
        let xj = getitem(interp, x, j.clone())?;
        let xi = getitem(interp, x, Value::int(i as i64))?;
        setitem(interp, x, Value::int(i as i64), xj)?;
        setitem(interp, x, j, xi)?;
    }
    Ok(())
}

/// `isinstance(population, collections.abc.Sequence)`.
fn is_sequence(interp: &mut Interpreter, population: &Value) -> Result<bool> {
    match population.kind() {
        ValueKind::List(_)
        | ValueKind::Tuple(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Range { .. }
        | ValueKind::BigRange { .. } => return Ok(true),
        ValueKind::Dict(_) | ValueKind::Set(_) | ValueKind::None => return Ok(false),
        _ => {}
    }
    let abc = interp.load_module("_collections_abc")?;
    let sequence = interp.get_attr(&abc, "Sequence")?;
    let result = builtin(interp, "isinstance", &[population.clone(), sequence])?;
    super::support::truthy(interp, &result)
}

/// `list(itertools.accumulate(iterable))`.
pub(super) fn accumulate(interp: &mut Interpreter, iterable: &Value) -> Result<Vec<Value>> {
    let items = interp.collect_iterable(iterable)?;
    let mut out: Vec<Value> = Vec::with_capacity(items.len());
    for item in items {
        let next = match out.last() {
            None => item,
            Some(total) => match add_fast(total, &item) {
                Some(sum) => sum,
                None => binop(interp, total, BinaryOp::Add, &item)?,
            },
        };
        out.push(next);
    }
    Ok(out)
}

/// `a + b` for two exact `int` / `float` / `bool` values whose Python sum the
/// native arithmetic reproduces.
fn add_fast(a: &Value, b: &Value) -> Option<Value> {
    match (a.kind(), b.kind()) {
        (ValueKind::Float(x), ValueKind::Float(y)) => Some(Value::float(x + y)),
        (ValueKind::Float(x), _) => exact_int(b).map(|y| Value::float(x + y as f64)),
        (_, ValueKind::Float(y)) => exact_int(a).map(|x| Value::float(x as f64 + y)),
        _ => {
            let (x, y) = (exact_int(a)?, exact_int(b)?);
            x.checked_add(y).map(Value::int)
        }
    }
}

/// `bisect.bisect_right(a, x, lo, hi)`.
fn bisect_right(
    interp: &mut Interpreter,
    a: &Value,
    x: &Value,
    mut lo: usize,
    mut hi: usize,
) -> Result<usize> {
    while lo < hi {
        let mid = (lo + hi) / 2;
        let item = getitem_at(interp, a, mid)?;
        if compare(interp, x, BinaryOp::Lt, &item)? {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(lo)
}

/// The values of a list of exact `int` / `float` values, as `f64`, when every
/// one converts without rounding (so comparing a float to it natively is
/// exact).
fn exact_floats(items: &[Value]) -> Option<Vec<f64>> {
    items
        .iter()
        .map(|v| match v.kind() {
            ValueKind::Float(f) => Some(f),
            ValueKind::Int(n) if n.unsigned_abs() <= 1 << 53 => Some(n as f64),
            ValueKind::Bool(b) => Some(f64::from(u8::from(b))),
            _ => None,
        })
        .collect()
}

/// `bisect_right` over a sorted `f64` table.
fn bisect_floats(table: &[f64], x: f64, hi: usize) -> usize {
    let (mut lo, mut hi) = (0, hi);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if x < table[mid] {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// `sample(population, k, *, counts=None)`.
pub(super) fn sample(
    interp: &mut Interpreter,
    this: &Value,
    population: &Value,
    k: &Value,
    counts: Option<&Value>,
) -> Result<Value> {
    if !is_sequence(interp, population)? {
        return Err(type_error(
            "Population must be a sequence.  For dicts or sets, use sorted(d).",
        ));
    }
    let n = length(interp, population)?;
    if let Some(counts) = counts.filter(|c| !c.is_none()) {
        return sample_counts(interp, this, population, n, k, counts);
    }
    let randbelow = below(interp, this)?;
    let in_range = compare(interp, &Value::int(0), BinaryOp::Le, k)?
        && compare(interp, k, BinaryOp::Le, &Value::int(n as i64))?;
    if !in_range {
        return Err(value_error("Sample larger than population or is negative"));
    }
    let k = match exact_int(k) {
        Some(k) => k as usize,
        None => {
            // `[None] * k`: a non-`int` raises CPython's sequence-repeat error.
            let repeated = binop(interp, &Value::list(vec![Value::none()]), BinaryOp::Mul, k)?;
            length(interp, &repeated)?
        }
    };
    let mut setsize: u128 = 21;
    if k > 5 {
        let exponent = ((k * 3) as f64).ln() / 4.0_f64.ln();
        setsize += 4_u128.saturating_pow(exponent.ceil() as u32);
    }
    let mut result: Vec<Value> = Vec::with_capacity(k);
    if (n as u128) <= setsize {
        let mut pool = interp.collect_iterable(population)?;
        for i in 0..k {
            let j = pick(interp, this, &randbelow, n - i)?;
            let j = list_index(interp, &j, pool.len())?;
            result.push(pool[j].clone());
            let last = n - i - 1;
            let moved = pool
                .get(last)
                .cloned()
                .ok_or_else(|| index_error("list index out of range"))?;
            pool[j] = moved;
        }
    } else if randbelow.is_native() {
        let mut selected: HashSet<u64> = HashSet::with_capacity(k);
        for _ in 0..k {
            let mut j = randbelow.below_native(this, n as u64)?;
            while selected.contains(&j) {
                j = randbelow.below_native(this, n as u64)?;
            }
            selected.insert(j);
            result.push(getitem_at(interp, population, j as usize)?);
        }
    } else {
        let selected = builtin(interp, "set", &[])?;
        let add = interp.get_attr(&selected, "add")?;
        let population_len = Value::int(n as i64);
        for _ in 0..k {
            let mut j = randbelow.call(interp, this, &population_len)?;
            while compare(interp, &j, BinaryOp::In, &selected)? {
                j = randbelow.call(interp, this, &population_len)?;
            }
            call(interp, add.clone(), std::slice::from_ref(&j))?;
            result.push(getitem(interp, population, j)?);
        }
    }
    Ok(Value::list(result))
}

/// `randbelow(n)` for `n > 0` as a Python object.
fn pick(interp: &mut Interpreter, this: &Value, randbelow: &Below, n: usize) -> Result<Value> {
    if randbelow.is_native()
        && (matches!(randbelow, Below::With(_)) || (n as u64) < super::dispatch::MAXSIZE)
    {
        return Ok(u64_value(randbelow.below_native(this, n as u64)?));
    }
    randbelow.call(interp, this, &Value::int(n as i64))
}

/// A `list` index: `__index__`, negative indices from the end, and
/// `IndexError` out of range.
fn list_index(interp: &mut Interpreter, j: &Value, len: usize) -> Result<usize> {
    let raw = match exact_int(j) {
        Some(j) => BigInt::from(j),
        None => {
            let index = interp.call_index_protocol(j, "list")?;
            int_value_of(&index).unwrap_or_default()
        }
    };
    let len_big = BigInt::from(len);
    let adjusted = if raw.is_negative() {
        raw + &len_big
    } else {
        raw
    };
    if adjusted.is_negative() || adjusted >= len_big {
        return Err(index_error("list index out of range"));
    }
    Ok(adjusted.to_usize().unwrap_or_default())
}

fn sample_counts(
    interp: &mut Interpreter,
    this: &Value,
    population: &Value,
    n: usize,
    k: &Value,
    counts: &Value,
) -> Result<Value> {
    let mut cum_counts = accumulate(interp, counts)?;
    if cum_counts.len() != n {
        return Err(value_error(
            "The number of counts does not match the population",
        ));
    }
    let total = cum_counts
        .pop()
        .ok_or_else(|| index_error("pop from empty list"))?;
    if !is_int(&total) {
        return Err(type_error("Counts must be integers"));
    }
    if compare(interp, &total, BinaryOp::Le, &Value::int(0))? {
        return Err(value_error("Total of counts must be greater than zero"));
    }
    let range = builtin(interp, "range", &[total])?;
    let sample_fn = interp.get_attr(this, "sample")?;
    let selections = call_kw(interp, sample_fn, &[range], &[("k", k.clone())])?;
    let selections = interp.collect_iterable(&selections)?;
    let table = exact_floats(&cum_counts);
    let cum_value = Value::list(cum_counts);
    let hi = length(interp, &cum_value)?;
    let mut out = Vec::with_capacity(selections.len());
    for s in selections {
        let i = match (&table, float_operand(&s)) {
            (Some(table), Some(x)) if matches!(s.kind(), ValueKind::Int(_)) => {
                bisect_floats(table, x, hi)
            }
            _ => bisect_right(interp, &cum_value, &s, 0, hi)?,
        };
        out.push(getitem_at(interp, population, i)?);
    }
    Ok(Value::list(out))
}

/// `choices(population, weights=None, *, cum_weights=None, k=1)`.
pub(super) fn choices(
    interp: &mut Interpreter,
    this: &Value,
    population: &Value,
    weights: Option<&Value>,
    cum_weights: Option<&Value>,
    k: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    let n = length(interp, population)?;
    let weights = weights.filter(|w| !w.is_none());
    let cum_weights = cum_weights.filter(|w| !w.is_none());
    let (cum, cum_value) = match (cum_weights, weights) {
        (None, None) => {
            let count = repeat_count(interp, k)?;
            let nf = n as f64;
            let mut out = Vec::with_capacity(count);
            for _ in 0..count {
                let i = match &src {
                    RandomSrc::Py(f) => {
                        let r = call(interp, f.clone(), &[])?;
                        let scaled = binop(interp, &r, BinaryOp::Mul, &Value::float(nf))?;
                        let floored = super::dispatch::floor(interp, &scaled)?;
                        out.push(getitem(interp, population, floored)?);
                        continue;
                    }
                    native => (native.draw_f64(this)? * nf).floor(),
                };
                out.push(getitem_at(interp, population, i as usize)?);
            }
            return Ok(Value::list(out));
        }
        (None, Some(weights)) => {
            let weights_is_int = is_int(weights);
            let cum = match super::support::try_swallow(
                || accumulate(interp, weights),
                |err| weights_is_int && err.class_name_is("TypeError"),
            )? {
                Ok(cum) => cum,
                Err(_) => {
                    let k = super::support::repr_value(interp, weights)?;
                    return Err(type_error(format!(
                        "The number of choices must be a keyword argument: k={k}"
                    )));
                }
            };
            let value = Value::list(cum.clone());
            (Some(cum), value)
        }
        (Some(_), Some(_)) => {
            return Err(type_error(
                "Cannot specify both weights and cumulative weights",
            ));
        }
        (Some(cum_weights), None) => {
            let items = match cum_weights.kind() {
                ValueKind::List(items) => Some(items.to_vec()),
                ValueKind::Tuple(items) => Some(items.to_vec()),
                _ => None,
            };
            (items, cum_weights.clone())
        }
    };
    if length(interp, &cum_value)? != n {
        return Err(value_error(
            "The number of weights does not match the population",
        ));
    }
    let last = getitem(interp, &cum_value, Value::int(-1))?;
    let total = binop(interp, &last, BinaryOp::Add, &Value::float(0.0))?;
    if compare(interp, &total, BinaryOp::Le, &Value::float(0.0))? {
        return Err(value_error("Total of weights must be greater than zero"));
    }
    let total_f = match total.kind() {
        ValueKind::Float(f) => Some(f),
        _ => None,
    };
    let finite = match total_f {
        Some(f) => f.is_finite(),
        None => {
            let r = call(
                interp,
                Value::builtin_function("math.isfinite"),
                std::slice::from_ref(&total),
            )?;
            super::support::truthy(interp, &r)?
        }
    };
    if !finite {
        return Err(value_error("Total of weights must be finite"));
    }
    let hi = n - 1;
    let count = repeat_count(interp, k)?;
    let table = cum.as_deref().and_then(exact_floats);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let i = match (&src, &table, total_f) {
            (RandomSrc::Mt | RandomSrc::Sys, Some(table), Some(total)) => {
                let x = src.draw_f64(this)? * total;
                bisect_floats(table, x, hi)
            }
            _ => {
                let r = src.draw(interp, this)?;
                let x = binop(interp, &r, BinaryOp::Mul, &total)?;
                bisect_right(interp, &cum_value, &x, 0, hi)?
            }
        };
        out.push(getitem_at(interp, population, i)?);
    }
    Ok(Value::list(out))
}

/// The `times` of `itertools.repeat(None, k)`: `__index__`, negative means 0.
fn repeat_count(interp: &mut Interpreter, k: &Value) -> Result<usize> {
    let k = index(interp, k)?;
    if k.is_negative() || k.is_zero() {
        return Ok(0);
    }
    k.to_isize().map(|k| k as usize).ok_or_else(|| {
        crate::error::PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}
