//! `statistics.NormalDist`: a port of the CPython 3.12.3 class.  An instance
//! keeps its parameters in the `_mu` / `_sigma` attributes (CPython's two
//! slots); every method re-reads them, so subclasses and `__setstate__`
//! behave as in CPython.

use super::functions::mean_stdev;
use super::ops::{
    binop, builtin, call, call_method, compare, is_normal_dist, items, math_call, math_f64,
    normal_dist_class, repr, stats_error, to_f64, type_error, value_error,
};
use crate::ast::BinaryOp;
use crate::error::Result;
use crate::interpreter::{Interpreter, value_class};
use crate::value::Value;

/// `_SQRT2 = sqrt(2.0)`.
pub(super) const SQRT2: f64 = std::f64::consts::SQRT_2;
/// `math.tau`.
const TAU: f64 = std::f64::consts::TAU;

fn mu_of(interp: &mut Interpreter, this: &Value) -> Result<f64> {
    let mu = interp.get_attr(this, "_mu")?;
    to_f64(interp, &mu)
}

fn sigma_of(interp: &mut Interpreter, this: &Value) -> Result<f64> {
    let sigma = interp.get_attr(this, "_sigma")?;
    to_f64(interp, &sigma)
}

/// `NormalDist(mu, sigma)` of the module's class.
fn make(interp: &mut Interpreter, mu: Value, sigma: Value) -> Result<Value> {
    let class = normal_dist_class()?;
    call(interp, class, &[mu, sigma])
}

/// `NormalDist.__init__(self, mu=0.0, sigma=1.0)`.
pub(super) fn init(
    interp: &mut Interpreter,
    this: &Value,
    mu: &Value,
    sigma: &Value,
) -> Result<()> {
    if compare(interp, sigma, BinaryOp::Lt, &Value::float(0.0))? {
        return Err(stats_error("sigma must be non-negative"));
    }
    let mu = builtin(interp, "float", std::slice::from_ref(mu))?;
    interp.assign_attr(this.clone(), "_mu", mu)?;
    let sigma = builtin(interp, "float", std::slice::from_ref(sigma))?;
    interp.assign_attr(this.clone(), "_sigma", sigma)?;
    Ok(())
}

/// `NormalDist.from_samples(cls, data)`.
pub(super) fn from_samples(interp: &mut Interpreter, cls: &Value, data: &Value) -> Result<Value> {
    let (mean, stdev) = mean_stdev(interp, data)?;
    call(interp, cls.clone(), &[mean, stdev])
}

/// `NormalDist.samples(self, n, *, seed=None)`.
pub(super) fn samples(
    interp: &mut Interpreter,
    this: &Value,
    n: &Value,
    seed: &Value,
) -> Result<Value> {
    let random = interp.load_module("random")?;
    let gauss = if seed.is_none() {
        interp.get_attr(&random, "gauss")?
    } else {
        let generator = interp.get_attr(&random, "Random")?;
        let generator = call(interp, generator, std::slice::from_ref(seed))?;
        interp.get_attr(&generator, "gauss")?
    };
    let mu = interp.get_attr(this, "_mu")?;
    let sigma = interp.get_attr(this, "_sigma")?;
    let repeat = super::ops::module_attr(interp, "itertools", "repeat")?;
    let counter = call(interp, repeat, &[Value::none(), n.clone()])?;
    let mut result = Vec::new();
    for _ in items(interp, &counter)? {
        result.push(call(interp, gauss.clone(), &[mu.clone(), sigma.clone()])?);
    }
    Ok(Value::list(result))
}

/// `NormalDist.pdf(self, x)`.
pub(super) fn pdf(interp: &mut Interpreter, this: &Value, x: &Value) -> Result<Value> {
    let sigma = sigma_of(interp, this)?;
    let variance = sigma * sigma;
    if variance == 0.0 {
        return Err(stats_error("pdf() not defined when sigma is zero"));
    }
    let mu = mu_of(interp, this)?;
    let diff = binop(interp, x, BinaryOp::Sub, &Value::float(mu))?;
    let square = binop(interp, &diff, BinaryOp::Mul, &diff)?;
    let exponent = binop(
        interp,
        &square,
        BinaryOp::Div,
        &Value::float(-2.0 * variance),
    )?;
    let numerator = math_call(interp, "exp", &[exponent])?;
    let denominator = math_call(interp, "sqrt", &[Value::float(TAU * variance)])?;
    binop(interp, &numerator, BinaryOp::Div, &denominator)
}

/// `NormalDist.cdf(self, x)`.
pub(super) fn cdf(interp: &mut Interpreter, this: &Value, x: &Value) -> Result<Value> {
    let sigma = sigma_of(interp, this)?;
    if sigma == 0.0 {
        return Err(stats_error("cdf() not defined when sigma is zero"));
    }
    let mu = mu_of(interp, this)?;
    let diff = binop(interp, x, BinaryOp::Sub, &Value::float(mu))?;
    let scaled = binop(interp, &diff, BinaryOp::Div, &Value::float(sigma * SQRT2))?;
    let erf = math_f64(interp, "erf", &[scaled])?;
    Ok(Value::float(0.5 * (1.0 + erf)))
}

/// `NormalDist.inv_cdf(self, p)`.
pub(super) fn inv_cdf(interp: &mut Interpreter, this: &Value, p: &Value) -> Result<Value> {
    if compare(interp, p, BinaryOp::Le, &Value::float(0.0))?
        || compare(interp, p, BinaryOp::Ge, &Value::float(1.0))?
    {
        return Err(stats_error("p must be in the range 0.0 < p < 1.0"));
    }
    let mu = mu_of(interp, this)?;
    let sigma = sigma_of(interp, this)?;
    let p = to_f64(interp, p)?;
    crate::builtin_modules::internal_shims::normal_dist_inv_cdf(p, mu, sigma).map(Value::float)
}

/// `NormalDist.quantiles(self, n=4)`.
pub(super) fn quantiles(interp: &mut Interpreter, this: &Value, n: &Value) -> Result<Value> {
    let range = builtin(interp, "range", &[Value::int(1), n.clone()])?;
    let mut result = Vec::new();
    for i in items(interp, &range)? {
        let p = binop(interp, &i, BinaryOp::Div, n)?;
        result.push(call_method(interp, this, "inv_cdf", &[p])?);
    }
    Ok(Value::list(result))
}

/// `NormalDist.overlap(self, other)`.
pub(super) fn overlap(interp: &mut Interpreter, this: &Value, other: &Value) -> Result<Value> {
    if !is_normal_dist(other) {
        return Err(type_error("Expected another NormalDist instance"));
    }
    let (mut x, mut y) = (this.clone(), other.clone());
    let (x_sigma, x_mu) = (sigma_of(interp, &x)?, mu_of(interp, &x)?);
    let (y_sigma, y_mu) = (sigma_of(interp, &y)?, mu_of(interp, &y)?);
    // `(Y._sigma, Y._mu) < (X._sigma, X._mu)`: sort to assure commutativity.
    let y_first = if y_sigma != x_sigma {
        y_sigma < x_sigma
    } else {
        y_mu < x_mu
    };
    if y_first {
        std::mem::swap(&mut x, &mut y);
    }
    let (xs, xm) = (sigma_of(interp, &x)?, mu_of(interp, &x)?);
    let (ys, ym) = (sigma_of(interp, &y)?, mu_of(interp, &y)?);
    let x_var = interp.get_attr(&x, "variance")?;
    let x_var = to_f64(interp, &x_var)?;
    let y_var = interp.get_attr(&y, "variance")?;
    let y_var = to_f64(interp, &y_var)?;
    if x_var == 0.0 || y_var == 0.0 {
        return Err(stats_error("overlap() not defined when sigma is zero"));
    }
    let dv = y_var - x_var;
    let dm = (ym - xm).abs();
    if dv == 0.0 {
        let erf = math_f64(interp, "erf", &[Value::float(dm / (2.0 * xs * SQRT2))])?;
        return Ok(Value::float(1.0 - erf));
    }
    let a = xm * y_var - ym * x_var;
    let log = math_f64(interp, "log", &[Value::float(y_var / x_var)])?;
    let root = math_f64(interp, "sqrt", &[Value::float(dm * dm + dv * log)])?;
    let b = xs * ys * root;
    let x1 = (a + b) / dv;
    let x2 = (a - b) / dv;
    let cdf_of = |interp: &mut Interpreter, who: &Value, at: f64| -> Result<f64> {
        let value = call_method(interp, who, "cdf", &[Value::float(at)])?;
        to_f64(interp, &value)
    };
    let d1 = (cdf_of(interp, &y, x1)? - cdf_of(interp, &x, x1)?).abs();
    let d2 = (cdf_of(interp, &y, x2)? - cdf_of(interp, &x, x2)?).abs();
    Ok(Value::float(1.0 - (d1 + d2)))
}

/// `NormalDist.zscore(self, x)`.
pub(super) fn zscore(interp: &mut Interpreter, this: &Value, x: &Value) -> Result<Value> {
    let sigma = sigma_of(interp, this)?;
    if sigma == 0.0 {
        return Err(stats_error("zscore() not defined when sigma is zero"));
    }
    let mu = mu_of(interp, this)?;
    let diff = binop(interp, x, BinaryOp::Sub, &Value::float(mu))?;
    binop(interp, &diff, BinaryOp::Div, &Value::float(sigma))
}

/// `NormalDist.variance`: `self._sigma * self._sigma`.
pub(super) fn variance(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let sigma = sigma_of(interp, this)?;
    Ok(Value::float(sigma * sigma))
}

/// `NormalDist.__add__` / `__radd__`.
pub(super) fn add(interp: &mut Interpreter, x1: &Value, x2: &Value) -> Result<Value> {
    let mu = interp.get_attr(x1, "_mu")?;
    let sigma = interp.get_attr(x1, "_sigma")?;
    if is_normal_dist(x2) {
        let other_mu = interp.get_attr(x2, "_mu")?;
        let other_sigma = interp.get_attr(x2, "_sigma")?;
        let mu = binop(interp, &mu, BinaryOp::Add, &other_mu)?;
        let hypot = math_call(interp, "hypot", &[sigma, other_sigma])?;
        return make(interp, mu, hypot);
    }
    let mu = binop(interp, &mu, BinaryOp::Add, x2)?;
    make(interp, mu, sigma)
}

/// `NormalDist.__sub__`.
pub(super) fn sub(interp: &mut Interpreter, x1: &Value, x2: &Value) -> Result<Value> {
    let mu = interp.get_attr(x1, "_mu")?;
    let sigma = interp.get_attr(x1, "_sigma")?;
    if is_normal_dist(x2) {
        let other_mu = interp.get_attr(x2, "_mu")?;
        let other_sigma = interp.get_attr(x2, "_sigma")?;
        let mu = binop(interp, &mu, BinaryOp::Sub, &other_mu)?;
        let hypot = math_call(interp, "hypot", &[sigma, other_sigma])?;
        return make(interp, mu, hypot);
    }
    let mu = binop(interp, &mu, BinaryOp::Sub, x2)?;
    make(interp, mu, sigma)
}

/// `NormalDist.__mul__` / `__rmul__`.
pub(super) fn mul(interp: &mut Interpreter, x1: &Value, x2: &Value) -> Result<Value> {
    let mu = interp.get_attr(x1, "_mu")?;
    let sigma = interp.get_attr(x1, "_sigma")?;
    let mu = binop(interp, &mu, BinaryOp::Mul, x2)?;
    let magnitude = math_call(interp, "fabs", std::slice::from_ref(x2))?;
    let sigma = binop(interp, &sigma, BinaryOp::Mul, &magnitude)?;
    make(interp, mu, sigma)
}

/// `NormalDist.__truediv__`.
pub(super) fn truediv(interp: &mut Interpreter, x1: &Value, x2: &Value) -> Result<Value> {
    let mu = interp.get_attr(x1, "_mu")?;
    let sigma = interp.get_attr(x1, "_sigma")?;
    let mu = binop(interp, &mu, BinaryOp::Div, x2)?;
    let magnitude = math_call(interp, "fabs", std::slice::from_ref(x2))?;
    let sigma = binop(interp, &sigma, BinaryOp::Div, &magnitude)?;
    make(interp, mu, sigma)
}

/// `NormalDist.__pos__`.
pub(super) fn pos(interp: &mut Interpreter, x1: &Value) -> Result<Value> {
    let mu = interp.get_attr(x1, "_mu")?;
    let sigma = interp.get_attr(x1, "_sigma")?;
    make(interp, mu, sigma)
}

/// `NormalDist.__neg__`.
pub(super) fn neg(interp: &mut Interpreter, x1: &Value) -> Result<Value> {
    let mu = interp.get_attr(x1, "_mu")?;
    let sigma = interp.get_attr(x1, "_sigma")?;
    let mu = interp.eval_unary(mu, crate::ast::UnaryOp::Neg)?;
    make(interp, mu, sigma)
}

/// `NormalDist.__rsub__`: `-(x1 - x2)`.
pub(super) fn rsub(interp: &mut Interpreter, x1: &Value, x2: &Value) -> Result<Value> {
    let difference = binop(interp, x1, BinaryOp::Sub, x2)?;
    interp.eval_unary(difference, crate::ast::UnaryOp::Neg)
}

/// `NormalDist.__eq__`.
pub(super) fn eq(interp: &mut Interpreter, x1: &Value, x2: &Value) -> Result<Value> {
    if !is_normal_dist(x2) {
        return Ok(Value::not_implemented());
    }
    let (mu1, mu2) = (interp.get_attr(x1, "_mu")?, interp.get_attr(x2, "_mu")?);
    if !compare(interp, &mu1, BinaryOp::Eq, &mu2)? {
        return Ok(Value::bool_(false));
    }
    let (s1, s2) = (
        interp.get_attr(x1, "_sigma")?,
        interp.get_attr(x2, "_sigma")?,
    );
    Ok(Value::bool_(compare(interp, &s1, BinaryOp::Eq, &s2)?))
}

/// `NormalDist.__hash__`: `hash((self._mu, self._sigma))`.
pub(super) fn hash(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let mu = interp.get_attr(this, "_mu")?;
    let sigma = interp.get_attr(this, "_sigma")?;
    builtin(interp, "hash", &[Value::tuple(vec![mu, sigma])])
}

/// `NormalDist.__repr__`.
pub(super) fn repr_of(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let class = value_class(this);
    let name = interp.get_attr(&class, "__name__")?;
    let mu = interp.get_attr(this, "_mu")?;
    let sigma = interp.get_attr(this, "_sigma")?;
    let (mu, sigma) = (repr(interp, &mu)?, repr(interp, &sigma)?);
    Ok(Value::string(format!(
        "{}(mu={mu}, sigma={sigma})",
        name.as_str().unwrap_or_default()
    )))
}

/// `NormalDist.__getstate__`.
pub(super) fn getstate(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let mu = interp.get_attr(this, "_mu")?;
    let sigma = interp.get_attr(this, "_sigma")?;
    Ok(Value::tuple(vec![mu, sigma]))
}

/// `NormalDist.__setstate__`: `self._mu, self._sigma = state`.
pub(super) fn setstate(interp: &mut Interpreter, this: &Value, state: &Value) -> Result<Value> {
    let parts = items(interp, state)?;
    match parts.len() {
        2 => {}
        n if n > 2 => return Err(value_error("too many values to unpack (expected 2)")),
        n => {
            return Err(value_error(format!(
                "not enough values to unpack (expected 2, got {n})"
            )));
        }
    }
    interp.assign_attr(this.clone(), "_mu", parts[0].clone())?;
    interp.assign_attr(this.clone(), "_sigma", parts[1].clone())?;
    Ok(Value::none())
}
