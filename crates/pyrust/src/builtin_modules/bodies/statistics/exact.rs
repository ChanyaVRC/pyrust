//! The exact-arithmetic core of CPython 3.12.3's `statistics`: `_exact_ratio`,
//! `_sum`, `_ss`, `_coerce`, `_convert` and the correctly rounded square
//! roots.  Sums are accumulated as `BigInt` rationals keyed by denominator —
//! the same partial sums CPython keeps in `dict`s of `int`s — so `int`,
//! `float`, `Fraction`, `Decimal` (and any type with `as_integer_ratio()` or
//! `numerator` / `denominator`) are summed without rounding, while a NaN or an
//! infinity is carried as the Python value the type itself produces.

use indexmap::IndexMap;
use num_bigint::BigInt;
use num_traits::{One, Signed, Zero};

use super::ops::{
    binop, builtin, call, call_method, int_value, is, items, module_attr, to_bigint, try_swallow,
    type_error, type_name,
};
use crate::ast::BinaryOp;
use crate::error::Result;
use crate::interpreter::{Interpreter, value_class};
use crate::value::{Value, ValueKind};

// ── Rationals ────────────────────────────────────────────────────────────────

/// An exact rational in lowest terms with a positive denominator.
#[derive(Clone, Debug)]
pub(super) struct Ratio {
    pub(super) num: BigInt,
    pub(super) den: BigInt,
}

fn gcd(a: &BigInt, b: &BigInt) -> BigInt {
    let mut a = a.abs();
    let mut b = b.abs();
    while !b.is_zero() {
        let r = &a % &b;
        a = b;
        b = r;
    }
    a
}

impl Ratio {
    /// `Fraction(num, den)`; `den` must be non-zero.
    pub(super) fn new(mut num: BigInt, mut den: BigInt) -> Self {
        if den.is_negative() {
            num = -num;
            den = -den;
        }
        let g = gcd(&num, &den);
        if !g.is_one() && !g.is_zero() {
            num /= &g;
            den /= &g;
        }
        Self { num, den }
    }

    pub(super) fn zero() -> Self {
        Self {
            num: BigInt::zero(),
            den: BigInt::one(),
        }
    }

    pub(super) fn add(&self, other: &Ratio) -> Ratio {
        Ratio::new(
            &self.num * &other.den + &other.num * &self.den,
            &self.den * &other.den,
        )
    }

    pub(super) fn sub(&self, other: &Ratio) -> Ratio {
        Ratio::new(
            &self.num * &other.den - &other.num * &self.den,
            &self.den * &other.den,
        )
    }

    pub(super) fn mul(&self, other: &Ratio) -> Ratio {
        Ratio::new(&self.num * &other.num, &self.den * &other.den)
    }

    /// `self / other`; `other` must be non-zero.
    pub(super) fn div(&self, other: &Ratio) -> Ratio {
        Ratio::new(&self.num * &other.den, &self.den * &other.num)
    }

    pub(super) fn from_int(n: impl Into<BigInt>) -> Ratio {
        Ratio {
            num: n.into(),
            den: BigInt::one(),
        }
    }

    pub(super) fn is_positive(&self) -> bool {
        self.num.is_positive()
    }
}

/// `float.as_integer_ratio()` of a finite float (`None` for NaN / infinity).
pub(super) fn float_ratio(f: f64) -> Option<(BigInt, BigInt)> {
    if !f.is_finite() {
        return None;
    }
    if f == 0.0 {
        return Some((BigInt::zero(), BigInt::one()));
    }
    let bits = f.to_bits();
    let negative = (bits >> 63) != 0;
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1u64 << 52) - 1);
    let (mut mantissa, mut exp) = if exponent == 0 {
        (fraction, -1074i64)
    } else {
        (fraction | (1u64 << 52), exponent - 1075)
    };
    let (num, den) = if exp >= 0 {
        (BigInt::from(mantissa) << (exp as usize), BigInt::one())
    } else {
        let shift = i64::from(mantissa.trailing_zeros()).min(-exp);
        mantissa >>= shift as u32;
        exp += shift;
        (BigInt::from(mantissa), BigInt::one() << ((-exp) as usize))
    };
    Some((if negative { -num } else { num }, den))
}

// ── `_exact_ratio` ────────────────────────────────────────────────────────────

pub(super) enum ExactRatio {
    /// `(numerator, denominator)`.
    Pair(BigInt, BigInt),
    /// `(x, None)`: a float NaN / infinity (or a `Decimal` one).
    Special,
}

fn int_of(value: &Value) -> Result<BigInt> {
    to_bigint(value).ok_or_else(|| {
        type_error(format!(
            "can't convert type '{}' to numerator/denominator",
            type_name(value)
        ))
    })
}

/// `_exact_ratio(x)`.
pub(super) fn exact_ratio(interp: &mut Interpreter, x: &Value) -> Result<ExactRatio> {
    match x.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => {
            return Ok(ExactRatio::Pair(
                to_bigint(x).unwrap_or_default(),
                BigInt::one(),
            ));
        }
        ValueKind::Float(f) => {
            return Ok(match float_ratio(f) {
                Some((n, d)) => ExactRatio::Pair(n, d),
                None => ExactRatio::Special,
            });
        }
        _ => {}
    }
    // try: return x.as_integer_ratio()
    let attempt = try_swallow(
        || {
            let method = interp.get_attr(x, "as_integer_ratio")?;
            call(interp, method, &[])
        },
        |err| {
            err.class_name_is("AttributeError")
                || err.class_name_is("OverflowError")
                || err.class_name_is("ValueError")
        },
    )?;
    match attempt {
        Ok(pair) => {
            let parts = pair
                .as_tuple()
                .map(<[Value]>::to_vec)
                .ok_or_else(|| type_error("as_integer_ratio() must return a tuple"))?;
            if parts.len() != 2 {
                return Err(type_error("as_integer_ratio() must return a 2-tuple"));
            }
            return Ok(ExactRatio::Pair(int_of(&parts[0])?, int_of(&parts[1])?));
        }
        Err(err) if err.class_name_is("AttributeError") => {}
        Err(_) => return Ok(ExactRatio::Special),
    }
    // try: return (x.numerator, x.denominator)
    let attempt = try_swallow(
        || {
            let numerator = interp.get_attr(x, "numerator")?;
            let denominator = interp.get_attr(x, "denominator")?;
            Ok((numerator, denominator))
        },
        |err| err.class_name_is("AttributeError"),
    )?;
    match attempt {
        Ok((numerator, denominator)) => {
            Ok(ExactRatio::Pair(int_of(&numerator)?, int_of(&denominator)?))
        }
        Err(_) => Err(type_error(format!(
            "can't convert type '{}' to numerator/denominator",
            type_name(x)
        ))),
    }
}

// ── Classes and `_coerce` ────────────────────────────────────────────────────

fn int_class() -> Value {
    value_class(&Value::int(0))
}

fn float_class() -> Value {
    value_class(&Value::float(0.0))
}

fn bool_class() -> Value {
    value_class(&Value::bool_(true))
}

fn issubclass(interp: &mut Interpreter, sub: &Value, sup: &Value) -> Result<bool> {
    let result = builtin(interp, "issubclass", &[sub.clone(), sup.clone()])?;
    super::ops::truthy(interp, &result)
}

/// `issubclass(T, Decimal)`; never imports `decimal` for the builtin types.
pub(super) fn is_decimal_subclass(interp: &mut Interpreter, t: &Value) -> Result<bool> {
    if is(t, &int_class()) || is(t, &float_class()) {
        return Ok(false);
    }
    let decimal = module_attr(interp, "decimal", "Decimal")?;
    issubclass(interp, t, &decimal)
}

/// `_coerce(T, S)`.
fn coerce(interp: &mut Interpreter, t: &Value, s: &Value) -> Result<Value> {
    let int = int_class();
    if is(t, s) {
        return Ok(t.clone());
    }
    if is(s, &int) || is(s, &bool_class()) {
        return Ok(t.clone());
    }
    if is(t, &int) {
        return Ok(s.clone());
    }
    if issubclass(interp, s, t)? {
        return Ok(s.clone());
    }
    if issubclass(interp, t, s)? {
        return Ok(t.clone());
    }
    if issubclass(interp, t, &int)? {
        return Ok(s.clone());
    }
    if issubclass(interp, s, &int)? {
        return Ok(t.clone());
    }
    let fraction = module_attr(interp, "fractions", "Fraction")?;
    let float = float_class();
    if issubclass(interp, t, &fraction)? && issubclass(interp, s, &float)? {
        return Ok(s.clone());
    }
    if issubclass(interp, t, &float)? && issubclass(interp, s, &fraction)? {
        return Ok(t.clone());
    }
    let name = |interp: &mut Interpreter, class: &Value| -> Result<String> {
        let name = interp.get_attr(class, "__name__")?;
        Ok(name.as_str().unwrap_or_default().to_string())
    };
    Err(type_error(format!(
        "don't know how to coerce {} and {}",
        name(interp, t)?,
        name(interp, s)?
    )))
}

/// `reduce(_coerce, types, int)`.
fn coerce_all(interp: &mut Interpreter, types: &[Value]) -> Result<Value> {
    let mut result = int_class();
    for s in types {
        result = coerce(interp, &result, s)?;
    }
    Ok(result)
}

fn note_type(types: &mut Vec<Value>, x: &Value) {
    let typ = value_class(x);
    if !types.iter().any(|seen| is(seen, &typ)) {
        types.push(typ);
    }
}

// ── Numbers: an exact rational or a Python value ─────────────────────────────

/// The result of an exact computation: a rational, or (for a NaN / infinity
/// input) the Python value the input's own type produced.
#[derive(Clone)]
pub(super) enum Num {
    Exact(Ratio),
    Py(Value),
}

impl Num {
    /// `self / n` for a positive count `n`.
    pub(super) fn div_int(&self, interp: &mut Interpreter, n: usize) -> Result<Num> {
        match self {
            Num::Exact(r) => Ok(Num::Exact(r.div(&Ratio::from_int(n)))),
            Num::Py(v) => Ok(Num::Py(binop(
                interp,
                v,
                BinaryOp::Div,
                &Value::int(n as i64),
            )?)),
        }
    }

    /// The Python object (`Fraction` for an exact rational).
    pub(super) fn to_value(&self, interp: &mut Interpreter) -> Result<Value> {
        match self {
            Num::Exact(r) => ratio_to_fraction(interp, r),
            Num::Py(v) => Ok(v.clone()),
        }
    }

    /// `self <= 0`.
    pub(super) fn is_non_positive(&self, interp: &mut Interpreter) -> Result<bool> {
        match self {
            Num::Exact(r) => Ok(!r.is_positive()),
            Num::Py(v) => super::ops::compare(interp, v, BinaryOp::Le, &Value::int(0)),
        }
    }
}

/// `Fraction(num, den)`.
pub(super) fn ratio_to_fraction(interp: &mut Interpreter, r: &Ratio) -> Result<Value> {
    let fraction = module_attr(interp, "fractions", "Fraction")?;
    call(
        interp,
        fraction,
        &[int_value(r.num.clone()), int_value(r.den.clone())],
    )
}

/// `float(Fraction)`: `numerator / denominator` with correct rounding.
fn ratio_to_float(interp: &mut Interpreter, r: &Ratio) -> Result<Value> {
    binop(
        interp,
        &int_value(r.num.clone()),
        BinaryOp::Div,
        &int_value(r.den.clone()),
    )
}

/// `a / b` for two [`Num`]s (`b` non-zero).
pub(super) fn num_div(interp: &mut Interpreter, a: &Num, b: &Num) -> Result<Num> {
    if let (Num::Exact(x), Num::Exact(y)) = (a, b) {
        return Ok(Num::Exact(x.div(y)));
    }
    let x = a.to_value(interp)?;
    let y = b.to_value(interp)?;
    Ok(Num::Py(binop(interp, &x, BinaryOp::Div, &y)?))
}

// ── `_convert` ────────────────────────────────────────────────────────────────

/// `_convert(value, T)`.
pub(super) fn convert(interp: &mut Interpreter, value: Num, t: &Value) -> Result<Value> {
    match value {
        Num::Exact(r) => {
            let int = int_class();
            if is(t, &int) {
                if r.den.is_one() {
                    return Ok(int_value(r.num));
                }
                return ratio_to_float(interp, &r);
            }
            if is(t, &float_class()) {
                return ratio_to_float(interp, &r);
            }
            let fraction = ratio_to_fraction(interp, &r)?;
            convert_py(interp, fraction, t)
        }
        Num::Py(value) => convert_py(interp, value, t),
    }
}

fn convert_py(interp: &mut Interpreter, value: Value, t: &Value) -> Result<Value> {
    if is(&value_class(&value), t) {
        // Covers `T` being `Fraction`, or `value` being a NaN / infinity.
        return Ok(value);
    }
    let mut t = t.clone();
    if issubclass(interp, &t, &int_class())? {
        let denominator = interp.get_attr(&value, "denominator")?;
        if super::ops::compare(interp, &denominator, BinaryOp::Ne, &Value::int(1))? {
            t = float_class();
        }
    }
    let attempt = try_swallow(
        || call(interp, t.clone(), std::slice::from_ref(&value)),
        |err| err.class_name_is("TypeError"),
    )?;
    match attempt {
        Ok(converted) => Ok(converted),
        Err(err) => {
            let decimal = module_attr(interp, "decimal", "Decimal")?;
            if issubclass(interp, &t, &decimal)? {
                let numerator = interp.get_attr(&value, "numerator")?;
                let denominator = interp.get_attr(&value, "denominator")?;
                let numerator = call(interp, t.clone(), &[numerator])?;
                let denominator = call(interp, t.clone(), &[denominator])?;
                binop(interp, &numerator, BinaryOp::Div, &denominator)
            } else {
                Err(err)
            }
        }
    }
}

// ── `_sum` ────────────────────────────────────────────────────────────────────

/// The state of `_sum`: partial sums by denominator, the set of types seen
/// and the count.
pub(super) struct SumAcc {
    count: usize,
    types: Vec<Value>,
    partials: IndexMap<BigInt, BigInt>,
    special: Option<Value>,
}

impl SumAcc {
    pub(super) fn new() -> Self {
        Self {
            count: 0,
            types: Vec::new(),
            partials: IndexMap::new(),
            special: None,
        }
    }

    pub(super) fn add(&mut self, interp: &mut Interpreter, x: &Value) -> Result<()> {
        note_type(&mut self.types, x);
        match exact_ratio(interp, x)? {
            ExactRatio::Pair(n, d) => {
                self.count += 1;
                *self.partials.entry(d).or_default() += n;
            }
            ExactRatio::Special => {
                self.count += 1;
                let previous = self.special.take().unwrap_or_else(|| Value::int(0));
                self.special = Some(binop(interp, &previous, BinaryOp::Add, x)?);
            }
        }
        Ok(())
    }

    /// `(T, total, count)`.
    pub(super) fn finish(self, interp: &mut Interpreter) -> Result<(Value, Num, usize)> {
        let total = match self.special {
            Some(special) => Num::Py(special),
            None => {
                let mut sum = Ratio::zero();
                for (d, n) in &self.partials {
                    sum = sum.add(&Ratio::new(n.clone(), d.clone()));
                }
                Num::Exact(sum)
            }
        };
        let t = coerce_all(interp, &self.types)?;
        Ok((t, total, self.count))
    }
}

/// `_sum(data)`.
pub(super) fn sum(interp: &mut Interpreter, data: &Value) -> Result<(Value, Num, usize)> {
    let values = items(interp, data)?;
    let mut acc = SumAcc::new();
    for x in &values {
        acc.add(interp, x)?;
    }
    acc.finish(interp)
}

// ── `_ss` ─────────────────────────────────────────────────────────────────────

pub(super) struct Ss {
    pub(super) t: Value,
    pub(super) ssd: Num,
    pub(super) c: Num,
    pub(super) count: usize,
}

/// `_ss(data, c)`: the exact mean and sum of square deviations.
pub(super) fn ss(interp: &mut Interpreter, data: &Value, c: Option<&Value>) -> Result<Ss> {
    let values = items(interp, data)?;
    if let Some(c) = c {
        let mut acc = SumAcc::new();
        for x in &values {
            let d = binop(interp, x, BinaryOp::Sub, c)?;
            let square = binop(interp, &d, BinaryOp::Mul, &d)?;
            acc.add(interp, &square)?;
        }
        let (t, ssd, count) = acc.finish(interp)?;
        return Ok(Ss {
            t,
            ssd,
            c: Num::Py(c.clone()),
            count,
        });
    }
    let mut count = 0usize;
    let mut types: Vec<Value> = Vec::new();
    let mut sx_partials: IndexMap<BigInt, BigInt> = IndexMap::new();
    let mut sxx_partials: IndexMap<BigInt, BigInt> = IndexMap::new();
    let mut sx_special: Option<Value> = None;
    let mut sxx_special: Option<Value> = None;
    for x in &values {
        note_type(&mut types, x);
        count += 1;
        match exact_ratio(interp, x)? {
            ExactRatio::Pair(n, d) => {
                *sxx_partials.entry(d.clone()).or_default() += &n * &n;
                *sx_partials.entry(d).or_default() += n;
            }
            ExactRatio::Special => {
                let zero = Value::int(0);
                let previous = sx_special.take().unwrap_or_else(|| zero.clone());
                sx_special = Some(binop(interp, &previous, BinaryOp::Add, x)?);
                let square = binop(interp, x, BinaryOp::Mul, x)?;
                let previous = sxx_special.take().unwrap_or(zero);
                sxx_special = Some(binop(interp, &previous, BinaryOp::Add, &square)?);
            }
        }
    }
    let (ssd, c) = if count == 0 {
        (Num::Exact(Ratio::zero()), Num::Exact(Ratio::zero()))
    } else if let Some(special) = sx_special {
        (Num::Py(special.clone()), Num::Py(special))
    } else {
        let mut sx = Ratio::zero();
        for (d, n) in &sx_partials {
            sx = sx.add(&Ratio::new(n.clone(), d.clone()));
        }
        let mut sxx = Ratio::zero();
        for (d, n) in &sxx_partials {
            sxx = sxx.add(&Ratio::new(n.clone(), d * d));
        }
        let count_ratio = Ratio::from_int(count);
        // This formula has poor numeric properties for floats,
        // but with fractions it is exact.
        let ssd = count_ratio.mul(&sxx).sub(&sx.mul(&sx)).div(&count_ratio);
        let c = sx.div(&count_ratio);
        (Num::Exact(ssd), Num::Exact(c))
    };
    let t = coerce_all(interp, &types)?;
    Ok(Ss { t, ssd, c, count })
}

// ── Correctly rounded square roots ───────────────────────────────────────────

/// `2 * sys.float_info.mant_dig + 3`.
const SQRT_BIT_WIDTH: i64 = 2 * 53 + 3;

/// `_integer_sqrt_of_frac_rto(n, m)`.
fn integer_sqrt_of_frac_rto(n: &BigInt, m: &BigInt) -> BigInt {
    let quotient = pyrust_core::bigint_divmod_floor(n, m).0;
    let a = quotient.sqrt();
    if &(&a * &a * m) != n {
        a | BigInt::one()
    } else {
        a
    }
}

/// `_float_sqrt_of_frac(n, m)`.
pub(super) fn float_sqrt_of_frac(
    interp: &mut Interpreter,
    n: &BigInt,
    m: &BigInt,
) -> Result<Value> {
    let bits = n.bits() as i64 - m.bits() as i64 - SQRT_BIT_WIDTH;
    let q = bits.div_euclid(2);
    let (numerator, denominator) = if q >= 0 {
        (
            integer_sqrt_of_frac_rto(n, &(m << (2 * q) as usize)) << q as usize,
            BigInt::one(),
        )
    } else {
        (
            integer_sqrt_of_frac_rto(&(n << (-2 * q) as usize), m),
            BigInt::one() << (-q) as usize,
        )
    };
    binop(
        interp,
        &int_value(numerator),
        BinaryOp::Div,
        &int_value(denominator),
    )
}

/// `_decimal_sqrt_of_frac(n, m)`.
pub(super) fn decimal_sqrt_of_frac(
    interp: &mut Interpreter,
    n: &BigInt,
    m: &BigInt,
) -> Result<Value> {
    let decimal = module_attr(interp, "decimal", "Decimal")?;
    let (mut n, mut m) = (n.clone(), m.clone());
    if !n.is_positive() {
        if n.is_zero() {
            return call(interp, decimal, &[Value::string("0.0")]);
        }
        n = -n;
        m = -m;
    }
    let ratio_of = |interp: &mut Interpreter, value: &Value| -> Result<(BigInt, BigInt)> {
        let pair = call_method(interp, value, "as_integer_ratio", &[])?;
        let parts = pair
            .as_tuple()
            .map(<[Value]>::to_vec)
            .ok_or_else(|| type_error("as_integer_ratio() must return a tuple"))?;
        if parts.len() != 2 {
            return Err(type_error("as_integer_ratio() must return a 2-tuple"));
        }
        Ok((int_of(&parts[0])?, int_of(&parts[1])?))
    };
    let dn = call(interp, decimal.clone(), &[int_value(n.clone())])?;
    let dm = call(interp, decimal, &[int_value(m.clone())])?;
    let quotient = binop(interp, &dn, BinaryOp::Div, &dm)?;
    let root = call_method(interp, &quotient, "sqrt", &[])?;
    let (nr, dr) = ratio_of(interp, &root)?;

    let plus = call_method(interp, &root, "next_plus", &[])?;
    let (np, dp) = ratio_of(interp, &plus)?;
    // test: n / m > ((root + plus) / 2) ** 2
    let lhs = BigInt::from(4) * &n * (&dr * &dp) * (&dr * &dp);
    let rhs_base = &dr * &np + &dp * &nr;
    if lhs > &m * &rhs_base * &rhs_base {
        return Ok(plus);
    }

    let minus = call_method(interp, &root, "next_minus", &[])?;
    let (nm, dm2) = ratio_of(interp, &minus)?;
    // test: n / m < ((root + minus) / 2) ** 2
    let lhs = BigInt::from(4) * &n * (&dr * &dm2) * (&dr * &dm2);
    let rhs_base = &dr * &nm + &dm2 * &nr;
    if lhs < &m * &rhs_base * &rhs_base {
        return Ok(minus);
    }
    Ok(root)
}

/// The `(numerator, denominator)` of `mss`: native for an exact rational,
/// `mss.numerator` / `mss.denominator` otherwise (an `AttributeError` for a
/// NaN / infinity, as in CPython).
pub(super) fn numerator_denominator(
    interp: &mut Interpreter,
    mss: &Num,
) -> Result<(BigInt, BigInt)> {
    match mss {
        Num::Exact(r) => Ok((r.num.clone(), r.den.clone())),
        Num::Py(value) => {
            let numerator = interp.get_attr(value, "numerator")?;
            let denominator = interp.get_attr(value, "denominator")?;
            Ok((int_of(&numerator)?, int_of(&denominator)?))
        }
    }
}

/// The square root of `mss` as `stdev` / `pstdev` return it: a `Decimal` for
/// `Decimal` data, a correctly rounded float otherwise.
pub(super) fn sqrt_of_mss(interp: &mut Interpreter, t: &Value, mss: &Num) -> Result<Value> {
    let decimal = is_decimal_subclass(interp, t)?;
    let (n, m) = numerator_denominator(interp, mss)?;
    if decimal {
        decimal_sqrt_of_frac(interp, &n, &m)
    } else {
        float_sqrt_of_frac(interp, &n, &m)
    }
}
