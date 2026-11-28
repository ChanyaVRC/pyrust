//! The bodies of `random.Random`'s seeding, state and distribution methods
//! and of `SystemRandom`'s generator methods.
//!
//! Each distribution runs on [`FloatOps`] when `self.random()` is a native
//! generator and every parameter is a float operand, and on [`ValueOps`]
//! (the Python objects themselves) otherwise.

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{Signed, ToPrimitive};

use crate::ast::BinaryOp;
use crate::builtin_modules::_random as core;
use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{Value, ValueKind};

use super::arith::{FloatOps, MathFn, ValueOps};
use super::dispatch::{self, NativeId, RandomSrc, Resolved, random_src, resolve};
use super::dists::{self, Tri};
use super::support::{
    bare_exception, bigint_value, binop, call, compare, exact_int, float_operand, index, is_int,
    str_value, truthy, try_swallow, type_error, value_error,
};

/// The float operands of `params` when `src` is native.
fn floats<const K: usize>(src: &RandomSrc, params: [&Value; K]) -> Option<[f64; K]> {
    if !src.is_native() {
        return None;
    }
    let mut out = [0.0; K];
    for (slot, param) in out.iter_mut().zip(params) {
        *slot = float_operand(param)?;
    }
    Some(out)
}

pub(super) fn uniform(
    interp: &mut Interpreter,
    this: &Value,
    a: &Value,
    b: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    if let Some([a, b]) = floats(&src, [a, b]) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return dists::uniform(ops, &a, &b).map(Value::float);
    }
    dists::uniform(
        &mut ValueOps {
            interp,
            this,
            src: &src,
        },
        a,
        b,
    )
}

pub(super) fn triangular(
    interp: &mut Interpreter,
    this: &Value,
    low: &Value,
    high: &Value,
    mode: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    let mode_f = if mode.is_none() {
        Some(None)
    } else {
        float_operand(mode).map(Some)
    };
    if let (Some([lo, hi]), Some(mode_f)) = (floats(&src, [low, high]), mode_f) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return Ok(match dists::triangular(ops, &lo, &hi, mode_f.as_ref())? {
            Tri::Value(x) => Value::float(x),
            Tri::Low => low.clone(),
        });
    }
    let mode = (!mode.is_none()).then_some(mode);
    let ops = &mut ValueOps {
        interp,
        this,
        src: &src,
    };
    Ok(match dists::triangular(ops, low, high, mode)? {
        Tri::Value(x) => x,
        Tri::Low => low.clone(),
    })
}

pub(super) fn normalvariate(
    interp: &mut Interpreter,
    this: &Value,
    mu: &Value,
    sigma: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    if let Some([mu, sigma]) = floats(&src, [mu, sigma]) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return dists::normalvariate(ops, &mu, &sigma).map(Value::float);
    }
    dists::normalvariate(
        &mut ValueOps {
            interp,
            this,
            src: &src,
        },
        mu,
        sigma,
    )
}

pub(super) fn gauss(
    interp: &mut Interpreter,
    this: &Value,
    mu: &Value,
    sigma: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    let z = interp.get_attr(this, "gauss_next")?;
    interp.assign_attr(this.clone(), "gauss_next", Value::none())?;
    let z_f = match z.kind() {
        ValueKind::None => Some(None),
        ValueKind::Float(f) => Some(Some(f)),
        _ => None,
    };
    if let (Some([mu, sigma]), Some(z_f)) = (floats(&src, [mu, sigma]), z_f) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        let z = match z_f {
            Some(z) => z,
            None => {
                let (z, next) = dists::gauss_pair(ops)?;
                ops.interp
                    .assign_attr(this.clone(), "gauss_next", Value::float(next))?;
                z
            }
        };
        return dists::affine(ops, &mu, &z, &sigma).map(Value::float);
    }
    let ops = &mut ValueOps {
        interp,
        this,
        src: &src,
    };
    let z = if z.is_none() {
        let (z, next) = dists::gauss_pair(ops)?;
        ops.interp.assign_attr(this.clone(), "gauss_next", next)?;
        z
    } else {
        z
    };
    dists::affine(ops, mu, &z, sigma)
}

/// `math.exp(value)`.
fn exp(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if let ValueKind::Float(x) = value.kind() {
        return MathFn::Exp.apply(x).map(Value::float);
    }
    call(
        interp,
        Value::builtin_function("math.exp"),
        std::slice::from_ref(value),
    )
}

pub(super) fn lognormvariate(
    interp: &mut Interpreter,
    this: &Value,
    mu: &Value,
    sigma: &Value,
) -> Result<Value> {
    let z = match resolve(interp, this, "normalvariate", &[NativeId::Normalvariate])? {
        Resolved::Native(_) => normalvariate(interp, this, mu, sigma)?,
        Resolved::Bound(f) => call(interp, f, &[mu.clone(), sigma.clone()])?,
    };
    exp(interp, &z)
}

pub(super) fn expovariate(interp: &mut Interpreter, this: &Value, lambd: &Value) -> Result<Value> {
    let src = random_src(interp, this)?;
    if let Some([lambd]) = floats(&src, [lambd]) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return dists::expovariate(ops, &lambd).map(Value::float);
    }
    dists::expovariate(
        &mut ValueOps {
            interp,
            this,
            src: &src,
        },
        lambd,
    )
}

pub(super) fn vonmisesvariate(
    interp: &mut Interpreter,
    this: &Value,
    mu: &Value,
    kappa: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    if let Some([mu, kappa]) = floats(&src, [mu, kappa]) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return dists::vonmisesvariate(ops, &mu, &kappa).map(Value::float);
    }
    dists::vonmisesvariate(
        &mut ValueOps {
            interp,
            this,
            src: &src,
        },
        mu,
        kappa,
    )
}

/// `x <= 0.0`.
fn non_positive(interp: &mut Interpreter, x: &Value) -> Result<bool> {
    compare(interp, x, BinaryOp::Le, &Value::float(0.0))
}

pub(super) fn gammavariate(
    interp: &mut Interpreter,
    this: &Value,
    alpha: &Value,
    beta: &Value,
) -> Result<Value> {
    if non_positive(interp, alpha)? || non_positive(interp, beta)? {
        return Err(value_error("gammavariate: alpha and beta must be > 0.0"));
    }
    let src = random_src(interp, this)?;
    if let Some([alpha, beta]) = floats(&src, [alpha, beta]) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return dists::gammavariate(ops, &alpha, &beta).map(Value::float);
    }
    dists::gammavariate(
        &mut ValueOps {
            interp,
            this,
            src: &src,
        },
        alpha,
        beta,
    )
}

/// `self.gammavariate(alpha, 1.0)`.
fn gamma_unit(interp: &mut Interpreter, this: &Value, alpha: &Value) -> Result<Value> {
    match resolve(interp, this, "gammavariate", &[NativeId::Gammavariate])? {
        Resolved::Native(_) => gammavariate(interp, this, alpha, &Value::float(1.0)),
        Resolved::Bound(f) => call(interp, f, &[alpha.clone(), Value::float(1.0)]),
    }
}

pub(super) fn betavariate(
    interp: &mut Interpreter,
    this: &Value,
    alpha: &Value,
    beta: &Value,
) -> Result<Value> {
    let y = gamma_unit(interp, this, alpha)?;
    let nonzero = match y.kind() {
        ValueKind::Float(f) => f != 0.0,
        _ => truthy(interp, &y)?,
    };
    if !nonzero {
        return Ok(Value::float(0.0));
    }
    let other = gamma_unit(interp, this, beta)?;
    if let (ValueKind::Float(y), ValueKind::Float(other)) = (y.kind(), other.kind()) {
        let sum = y + other;
        if sum != 0.0 {
            return Ok(Value::float(y / sum));
        }
    }
    let sum = binop(interp, &y, BinaryOp::Add, &other)?;
    binop(interp, &y, BinaryOp::Div, &sum)
}

pub(super) fn paretovariate(
    interp: &mut Interpreter,
    this: &Value,
    alpha: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    if let Some([alpha]) = floats(&src, [alpha]) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return dists::paretovariate(ops, &alpha).map(Value::float);
    }
    dists::paretovariate(
        &mut ValueOps {
            interp,
            this,
            src: &src,
        },
        alpha,
    )
}

pub(super) fn weibullvariate(
    interp: &mut Interpreter,
    this: &Value,
    alpha: &Value,
    beta: &Value,
) -> Result<Value> {
    let src = random_src(interp, this)?;
    if let Some([alpha, beta]) = floats(&src, [alpha, beta]) {
        let ops = &mut FloatOps {
            interp,
            this,
            src: &src,
        };
        return dists::weibullvariate(ops, &alpha, &beta).map(Value::float);
    }
    dists::weibullvariate(
        &mut ValueOps {
            interp,
            this,
            src: &src,
        },
        alpha,
        beta,
    )
}

pub(super) fn binomialvariate(
    interp: &mut Interpreter,
    this: &Value,
    n: &Value,
    p: &Value,
) -> Result<Value> {
    if compare(interp, n, BinaryOp::Lt, &Value::int(0))? {
        return Err(value_error("n must be non-negative"));
    }
    if compare(interp, p, BinaryOp::Le, &Value::float(0.0))?
        || compare(interp, p, BinaryOp::Ge, &Value::float(1.0))?
    {
        if compare(interp, p, BinaryOp::Eq, &Value::float(0.0))? {
            return Ok(Value::int(0));
        }
        if compare(interp, p, BinaryOp::Eq, &Value::float(1.0))? {
            return Ok(n.clone());
        }
        return Err(value_error("p must be in the range 0.0 <= p <= 1.0"));
    }
    let src = random_src(interp, this)?;
    if compare(interp, n, BinaryOp::Eq, &Value::int(1))? {
        let r = src.draw(interp, this)?;
        if let (ValueKind::Float(r), Some(p)) = (r.kind(), float_operand(p)) {
            return Ok(Value::int(i64::from(r < p)));
        }
        let lt = binop(interp, &r, BinaryOp::Lt, p)?;
        return Ok(bigint_value(index(interp, &lt)?));
    }
    if compare(interp, p, BinaryOp::Gt, &Value::float(0.5))? {
        let q = binop(interp, &Value::float(1.0), BinaryOp::Sub, p)?;
        let inner = match resolve(
            interp,
            this,
            "binomialvariate",
            &[NativeId::Binomialvariate],
        )? {
            Resolved::Native(_) => binomialvariate(interp, this, n, &q)?,
            Resolved::Bound(f) => call(interp, f, &[n.clone(), q])?,
        };
        return binop(interp, n, BinaryOp::Sub, &inner);
    }
    let small_n = matches!(n.kind(), ValueKind::Int(_))
        .then(|| float_operand(n))
        .flatten();
    let result = match (small_n, floats(&src, [p])) {
        (Some(n_f), Some([p_f])) => {
            let ops = &mut FloatOps {
                interp,
                this,
                src: &src,
            };
            dists::binomial(ops, &n_f, &p_f)?.map(|k| Value::int(k as i64))
        }
        _ => dists::binomial(
            &mut ValueOps {
                interp,
                this,
                src: &src,
            },
            n,
            p,
        )?,
    };
    result.ok_or_else(|| bare_exception(interp, "AssertionError"))
}

// ── Seeding and state ────────────────────────────────────────────────────────

/// The text of a `str` (or `str` subclass) value.
fn str_text(value: &Value) -> Option<String> {
    crate::builtin_modules::_string::str_text(value)
}

/// The data of a `bytes` (or `bytes` subclass) value.
fn bytes_data(value: &Value) -> Option<Vec<u8>> {
    match value.kind() {
        ValueKind::Bytes(data) => Some(data.as_ref().clone()),
        ValueKind::PyInstance(_) => {
            crate::interpreter::builtin_data_backing(value).and_then(|b| match b.kind() {
                ValueKind::Bytes(data) => Some(data.as_ref().clone()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// The data of a `bytearray` (or `bytearray` subclass) value.
fn bytearray_data(value: &Value) -> Option<Vec<u8>> {
    pyrust_builtins::bytearray::as_bytearray_snapshot(value).or_else(|| {
        crate::interpreter::builtin_data_backing(value)
            .and_then(|b| pyrust_builtins::bytearray::as_bytearray_snapshot(&b))
    })
}

fn is_float(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Float(_) => true,
        ValueKind::PyInstance(_) => crate::interpreter::builtin_data_backing(value)
            .is_some_and(|b| matches!(b.kind(), ValueKind::Float(_))),
        _ => false,
    }
}

/// The version 1 (Python 2) seed of a `str` / `bytes`: its code points
/// folded by `((1000003 * x) ^ c) & 0xFFFFFFFFFFFFFFFF`.
fn version1_seed(codepoints: &[u32]) -> Value {
    let mut x: u64 = codepoints.first().map_or(0, |c| u64::from(*c) << 7);
    for &c in codepoints {
        x = 1_000_003_u64.wrapping_mul(x) ^ u64::from(c);
    }
    x ^= codepoints.len() as u64;
    core::u64_value(x)
}

/// `int.from_bytes(a + _sha512(a).digest())`.
fn version2_seed(data: &[u8]) -> Value {
    let mut folded = data.to_vec();
    folded.extend_from_slice(&crate::builtin_modules::hashlib::sha512_digest(data));
    bigint_value(BigInt::from_biguint(
        Sign::Plus,
        BigUint::from_bytes_be(&folded),
    ))
}

/// `Random.seed(self, a=None, version=2)`.
pub(super) fn seed(
    interp: &mut Interpreter,
    this: &Value,
    a: Value,
    version: &Value,
) -> Result<()> {
    let is_str = str_text(&a).is_some();
    let a = if compare(interp, version, BinaryOp::Eq, &Value::int(1))?
        && (is_str || bytes_data(&a).is_some())
    {
        let codepoints: Vec<u32> = match str_text(&a) {
            Some(text) => pyrust_core::cesu8_codepoints(&text).collect(),
            None => bytes_data(&a)
                .unwrap_or_default()
                .into_iter()
                .map(u32::from)
                .collect(),
        };
        version1_seed(&codepoints)
    } else if compare(interp, version, BinaryOp::Eq, &Value::int(2))?
        && (is_str || bytes_data(&a).is_some() || bytearray_data(&a).is_some())
    {
        let data = if is_str {
            let encoded = super::support::call_method(interp, &a, "encode", &[])?;
            bytes_data(&encoded).unwrap_or_default()
        } else {
            bytes_data(&a)
                .or_else(|| bytearray_data(&a))
                .unwrap_or_default()
        };
        version2_seed(&data)
    } else if !(a.is_none()
        || is_int(&a)
        || is_float(&a)
        || is_str
        || bytes_data(&a).is_some()
        || bytearray_data(&a).is_some())
    {
        return Err(type_error(
            "The only supported seed types are: None,\nint, float, str, bytes, and bytearray.",
        ));
    } else {
        a
    };
    core::seed_object(interp, this, &a)?;
    interp.assign_attr(this.clone(), "gauss_next", Value::none())
}

/// `Random.getstate(self)`: `self.VERSION, super().getstate(), self.gauss_next`.
pub(super) fn getstate(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let version = interp.get_attr(this, "VERSION")?;
    let internal = core::getstate_of(this)?;
    let gauss_next = interp.get_attr(this, "gauss_next")?;
    Ok(Value::tuple(vec![version, internal, gauss_next]))
}

/// `version, internalstate, self.gauss_next = state`.
fn unpack_state(interp: &mut Interpreter, this: &Value, state: &Value) -> Result<Value> {
    let items = match state.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        ValueKind::List(items) => items.to_vec(),
        _ => interp.collect_iterable(state)?,
    };
    if items.len() < 3 {
        return Err(value_error(format!(
            "not enough values to unpack (expected 3, got {})",
            items.len()
        )));
    }
    if items.len() > 3 {
        return Err(value_error("too many values to unpack (expected 3)"));
    }
    interp.assign_attr(this.clone(), "gauss_next", items[2].clone())?;
    Ok(items[1].clone())
}

/// `Random.setstate(self, state)`.
pub(super) fn setstate(interp: &mut Interpreter, this: &Value, state: &Value) -> Result<()> {
    let version = super::support::getitem(interp, state, Value::int(0))?;
    if compare(interp, &version, BinaryOp::Eq, &Value::int(3))? {
        let internal = unpack_state(interp, this, state)?;
        return core::setstate_of(interp, this, &internal);
    }
    if compare(interp, &version, BinaryOp::Eq, &Value::int(2))? {
        let internal = unpack_state(interp, this, state)?;
        let modulus = bigint_value(BigInt::from(1_u64 << 32));
        let converted = try_swallow(
            || -> Result<Vec<Value>> {
                interp
                    .collect_iterable(&internal)?
                    .iter()
                    .map(|x| binop(interp, x, BinaryOp::Mod, &modulus))
                    .collect()
            },
            |err| err.class_name_is("ValueError"),
        )?;
        let converted = match converted {
            Ok(items) => Value::tuple(items),
            Err(_) => return Err(bare_exception(interp, "TypeError")),
        };
        return core::setstate_of(interp, this, &converted);
    }
    let own = interp.get_attr(this, "VERSION")?;
    let (version, own) = (str_value(interp, &version)?, str_value(interp, &own)?);
    Err(value_error(format!(
        "state with version {version} passed to Random.setstate() of version {own}"
    )))
}

/// `Random.__init_subclass__(cls, /, **kwargs)`: pick the `_randbelow` the
/// subclass's `random()` / `getrandbits()` can support.
pub(super) fn init_subclass(interp: &mut Interpreter, cls: &Value) -> Result<()> {
    let ValueKind::PyClass(class) = cls.kind() else {
        return Ok(());
    };
    let class = std::rc::Rc::clone(class);
    let mut choice = None;
    for c in crate::interpreter::c3_linearize_classes(&class) {
        let c = c.borrow();
        if c.attrs.contains_key("_randbelow") {
            break;
        }
        if c.attrs.contains_key("getrandbits") {
            choice = Some("_randbelow_with_getrandbits");
            break;
        }
        if c.attrs.contains_key("random") {
            choice = Some("_randbelow_without_getrandbits");
            break;
        }
    }
    if let Some(name) = choice {
        let method = interp.get_attr(cls, name)?;
        interp.assign_attr(cls.clone(), "_randbelow", method)?;
    }
    Ok(())
}

// ── SystemRandom ─────────────────────────────────────────────────────────────

/// `_urandom(n)`: `os.urandom`'s `Py_ssize_t` argument and range checks.
fn urandom_arg(interp: &mut Interpreter, n: &Value) -> Result<Vec<u8>> {
    let n = index(interp, n)?;
    if n.is_negative() {
        return Err(value_error("negative argument not allowed"));
    }
    let n = n.to_isize().ok_or_else(|| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })?;
    core::urandom(n as usize)
}

/// `SystemRandom.getrandbits(self, k)`.
pub(super) fn sys_getrandbits(interp: &mut Interpreter, k: &Value) -> Result<Value> {
    if compare(interp, k, BinaryOp::Lt, &Value::int(0))? {
        return Err(value_error("number of bits must be non-negative"));
    }
    if let Some(k) = exact_int(k) {
        return dispatch::sys_bits(k as u64);
    }
    let plus = binop(interp, k, BinaryOp::Add, &Value::int(7))?;
    let numbytes = binop(interp, &plus, BinaryOp::FloorDiv, &Value::int(8))?;
    let bytes = urandom_arg(interp, &numbytes)?;
    let x = bigint_value(BigInt::from_biguint(
        Sign::Plus,
        BigUint::from_bytes_be(&bytes),
    ));
    let bits = binop(interp, &numbytes, BinaryOp::Mul, &Value::int(8))?;
    let shift = binop(interp, &bits, BinaryOp::Sub, k)?;
    binop(interp, &x, BinaryOp::RShift, &shift)
}

/// `SystemRandom.randbytes(self, n)`: `_urandom(n)`.
pub(super) fn sys_randbytes(interp: &mut Interpreter, n: &Value) -> Result<Value> {
    urandom_arg(interp, n).map(Value::bytes)
}
