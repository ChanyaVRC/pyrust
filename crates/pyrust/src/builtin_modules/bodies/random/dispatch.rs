//! `self.<method>` resolution for the native `random.Random` methods.
//!
//! The CPython source reaches randomness only through `self.random()`,
//! `self.getrandbits()` and `self._randbelow()` (and a few other methods call
//! `self.randrange()`, `self.gammavariate()`, ...), so a subclass overriding
//! any of them changes every derived method.  [`resolve`] performs that
//! attribute lookup and reports when it still finds this module's own native
//! implementation, which the callers then run directly instead of making a
//! Python-level call.

use std::cell::RefCell;
use std::rc::Rc;

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::Zero;

use crate::ast::BinaryOp;
use crate::builtin_modules::_random::{self as core, u64_value, with_mt};
use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::Value;

use super::support::{bigint_value, binop, call, compare, exact_int, int_value_of, warn};

/// `2 ** BPF`: the default `maxsize` of `_randbelow_without_getrandbits`.
pub(super) const MAXSIZE: u64 = 1 << 53;

/// `RECIP_BPF` (`2 ** -53`).
pub(super) const RECIP_BPF: f64 = 1.0 / 9_007_199_254_740_992.0;

const RANDBELOW_WARNING: &str = "Underlying random() generator does not supply \nenough bits to choose from a population range this large.\nTo remove the range limitation, add a getrandbits() method.";

/// A native method that a lookup on `self` can find.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum NativeId {
    /// `_random.Random.random`.
    MtRandom,
    /// `_random.Random.getrandbits`.
    MtGetrandbits,
    /// `SystemRandom.random`.
    SysRandom,
    /// `SystemRandom.getrandbits`.
    SysGetrandbits,
    /// `Random._randbelow_with_getrandbits` (also bound as `_randbelow`).
    RandbelowWith,
    /// `Random._randbelow_without_getrandbits`.
    RandbelowWithout,
    Randrange,
    Normalvariate,
    Gammavariate,
    Binomialvariate,
}

// Per-interpreter state (`gil_local!`):
// (a) Why: deciding whether `self.random` (etc.) is still the native method
//     needs the function objects stored in the module's class dictionaries.
// (b) Every generation is kept (a re-import after `del sys.modules[...]`
//     creates new classes, and old instances keep working).
pyrust_core::gil_local! {
    static NATIVE: RefCell<Vec<(Value, NativeId)>> = const { RefCell::new(Vec::new()) };
}

/// Record `value` (a function stored in a class dictionary) as `id`.
pub(super) fn register(value: Value, id: NativeId) {
    NATIVE.with(|native| native.borrow_mut().push((value, id)));
}

fn native_id(raw: &Value) -> Option<NativeId> {
    NATIVE.with(|native| {
        native
            .borrow()
            .iter()
            .find(|(value, _)| value.is_identical_to(raw))
            .map(|(_, id)| *id)
    })
}

/// What `self.<name>` resolves to.
pub(super) enum Resolved {
    /// One of `allowed`: run it natively.
    Native(NativeId),
    /// Anything else: the bound attribute, to be called.
    Bound(Value),
}

/// `self.<name>`.  It is [`Resolved::Native`] when the instance dictionary
/// does not shadow `name` and the class attribute the MRO finds is the native
/// function registered as one of `allowed`.
pub(super) fn resolve(
    interp: &mut Interpreter,
    this: &Value,
    name: &str,
    allowed: &[NativeId],
) -> Result<Resolved> {
    if let Some(instance) = this.as_py_instance_rc() {
        let class = {
            let instance = instance.borrow();
            (!instance.attrs.contains_key(name)).then(|| Rc::clone(&instance.class))
        };
        if let Some(raw) =
            class.and_then(|class| crate::interpreter::lookup_class_attr(&class, name))
            && let Some(id) = native_id(&raw).filter(|id| allowed.contains(id))
        {
            return Ok(Resolved::Native(id));
        }
    }
    Ok(Resolved::Bound(interp.get_attr(this, name)?))
}

/// The source of `self.random()`.
pub(super) enum RandomSrc {
    Mt,
    Sys,
    Py(Value),
}

/// The source of `self.getrandbits(k)`.
pub(super) enum BitsSrc {
    Mt,
    Sys,
    Py(Value),
}

/// `self._randbelow`.
pub(super) enum Below {
    /// `_randbelow_with_getrandbits` over `self.getrandbits`.
    With(BitsSrc),
    /// `_randbelow_without_getrandbits` over `self.random`.
    Without(RandomSrc),
    /// Any other callable.
    Py(Value),
}

/// `random = self.random`.
pub(super) fn random_src(interp: &mut Interpreter, this: &Value) -> Result<RandomSrc> {
    Ok(
        match resolve(
            interp,
            this,
            "random",
            &[NativeId::MtRandom, NativeId::SysRandom],
        )? {
            Resolved::Native(NativeId::SysRandom) => RandomSrc::Sys,
            Resolved::Native(_) => RandomSrc::Mt,
            Resolved::Bound(f) => RandomSrc::Py(f),
        },
    )
}

/// `getrandbits = self.getrandbits`.
pub(super) fn bits_src(interp: &mut Interpreter, this: &Value) -> Result<BitsSrc> {
    Ok(
        match resolve(
            interp,
            this,
            "getrandbits",
            &[NativeId::MtGetrandbits, NativeId::SysGetrandbits],
        )? {
            Resolved::Native(NativeId::SysGetrandbits) => BitsSrc::Sys,
            Resolved::Native(_) => BitsSrc::Mt,
            Resolved::Bound(f) => BitsSrc::Py(f),
        },
    )
}

/// `randbelow = self._randbelow`.
pub(super) fn below(interp: &mut Interpreter, this: &Value) -> Result<Below> {
    Ok(
        match resolve(
            interp,
            this,
            "_randbelow",
            &[NativeId::RandbelowWith, NativeId::RandbelowWithout],
        )? {
            Resolved::Native(NativeId::RandbelowWithout) => {
                Below::Without(random_src(interp, this)?)
            }
            Resolved::Native(_) => Below::With(bits_src(interp, this)?),
            Resolved::Bound(f) => Below::Py(f),
        },
    )
}

/// `SystemRandom.random()`: `(int.from_bytes(_urandom(7)) >> 3) * RECIP_BPF`.
pub(super) fn sys_random() -> Result<f64> {
    let bytes = core::urandom(7)?;
    let mut word = [0_u8; 8];
    word[1..].copy_from_slice(&bytes);
    Ok((u64::from_be_bytes(word) >> 3) as f64 * RECIP_BPF)
}

/// `SystemRandom.getrandbits(k)` for `k >= 0`.
pub(super) fn sys_bits(k: u64) -> Result<Value> {
    let numbytes = k.div_ceil(8);
    let bytes = core::urandom(numbytes as usize)?;
    let x = BigUint::from_bytes_be(&bytes) >> (numbytes * 8 - k);
    Ok(bigint_value(BigInt::from_biguint(Sign::Plus, x)))
}

fn sys_bits_u64(k: u32) -> Result<u64> {
    let numbytes = k.div_ceil(8) as usize;
    let bytes = core::urandom(numbytes)?;
    let mut word = [0_u8; 8];
    word[8 - numbytes..].copy_from_slice(&bytes);
    Ok(u64::from_be_bytes(word) >> (numbytes as u32 * 8 - k))
}

impl RandomSrc {
    /// Runs no Python code (and so returns an exact `float`).
    pub(super) fn is_native(&self) -> bool {
        !matches!(self, RandomSrc::Py(_))
    }

    /// `random()` for a native source.
    pub(super) fn draw_f64(&self, this: &Value) -> Result<f64> {
        match self {
            RandomSrc::Sys => sys_random(),
            _ => with_mt(this, |mt| mt.random()),
        }
    }

    /// `random()`.
    pub(super) fn draw(&self, interp: &mut Interpreter, this: &Value) -> Result<Value> {
        match self {
            RandomSrc::Py(f) => call(interp, f.clone(), &[]),
            native => native.draw_f64(this).map(Value::float),
        }
    }
}

impl BitsSrc {
    fn is_native(&self) -> bool {
        !matches!(self, BitsSrc::Py(_))
    }

    /// `getrandbits(k)` for `0 < k <= 64` from a native source.
    fn bits_u64(&self, this: &Value, k: u32) -> Result<u64> {
        match self {
            BitsSrc::Sys => sys_bits_u64(k),
            _ => with_mt(this, |mt| mt.bits_u64(k)),
        }
    }

    /// `getrandbits(k)` (`k >= 0`) from a native source.
    fn bits_big(&self, this: &Value, k: u64) -> Result<BigInt> {
        match self {
            BitsSrc::Sys => Ok(int_value_of(&sys_bits(k)?).unwrap_or_default()),
            _ => with_mt(this, |mt| {
                if k == 0 {
                    BigInt::zero()
                } else {
                    BigInt::from(BigUint::new(mt.bits_words(k)))
                }
            }),
        }
    }

    /// `getrandbits(k)`.
    pub(super) fn bits(&self, interp: &mut Interpreter, this: &Value, k: u64) -> Result<Value> {
        match self {
            BitsSrc::Py(f) => call(interp, f.clone(), &[u64_value(k)]),
            BitsSrc::Sys => sys_bits(k),
            BitsSrc::Mt => with_mt(this, |mt| mt.getrandbits(k)),
        }
    }
}

/// `n.bit_length()` of a positive `u64`.
fn bit_length(n: u64) -> u32 {
    64 - n.leading_zeros()
}

impl Below {
    /// Runs no Python code: the receiver of the callers' fast paths (an exact
    /// `list` being shuffled, ...) cannot change while it runs.
    pub(super) fn is_native(&self) -> bool {
        match self {
            Below::With(bits) => bits.is_native(),
            Below::Without(src) => src.is_native(),
            Below::Py(_) => false,
        }
    }

    /// `randbelow(n)` for `0 < n`, as a `u64`, when [`Below::is_native`].
    pub(super) fn below_native(&self, this: &Value, n: u64) -> Result<u64> {
        match self {
            Below::With(BitsSrc::Mt) => {
                let k = bit_length(n);
                with_mt(this, |mt| {
                    loop {
                        let r = mt.bits_u64(k);
                        if r < n {
                            break r;
                        }
                    }
                })
            }
            Below::With(bits) => {
                let k = bit_length(n);
                loop {
                    let r = bits.bits_u64(this, k)?;
                    if r < n {
                        return Ok(r);
                    }
                }
            }
            Below::Without(src) if n < MAXSIZE => {
                let rem = MAXSIZE % n;
                let limit = (MAXSIZE - rem) as f64 / MAXSIZE as f64;
                let mut r = src.draw_f64(this)?;
                while r >= limit {
                    r = src.draw_f64(this)?;
                }
                Ok((r * MAXSIZE as f64).floor() as u64 % n)
            }
            _ => unreachable!("below_native requires a native source and n < 2**53"),
        }
    }

    /// `randbelow(n)` for a Python `int` (or any object a Python-level
    /// `_randbelow` override accepts).
    pub(super) fn call(&self, interp: &mut Interpreter, this: &Value, n: &Value) -> Result<Value> {
        if let Below::Py(f) = self {
            return call(interp, f.clone(), std::slice::from_ref(n));
        }
        if let Some(small) = exact_int(n).filter(|&n| n > 0)
            && self.is_native()
            && (matches!(self, Below::With(_)) || (small as u64) < MAXSIZE)
        {
            return Ok(u64_value(self.below_native(this, small as u64)?));
        }
        match self {
            Below::With(bits) => with_getrandbits(interp, this, bits, n),
            Below::Without(src) => without_getrandbits(interp, this, src, n, &u64_value(MAXSIZE)),
            Below::Py(_) => unreachable!("handled above"),
        }
    }
}

/// `_randbelow_with_getrandbits(self, n)` over a resolved `getrandbits`.
pub(super) fn with_getrandbits(
    interp: &mut Interpreter,
    this: &Value,
    bits: &BitsSrc,
    n: &Value,
) -> Result<Value> {
    if let Some(big) = int_value_of(n).filter(|n| n.sign() == Sign::Plus)
        && bits.is_native()
        && matches!(
            n.kind(),
            crate::value::ValueKind::Int(_) | crate::value::ValueKind::BigInt(_)
        )
    {
        if let Ok(small) = u64::try_from(&big) {
            return Ok(u64_value(
                Below::With(clone_bits(bits)).below_native(this, small)?,
            ));
        }
        let k = big.bits();
        loop {
            let r = bits.bits_big(this, k)?;
            if r < big {
                return Ok(bigint_value(r));
            }
        }
    }
    // `k = n.bit_length()`, then `getrandbits(k)` until `r < n` (a Python
    // `getrandbits`, a non-`int` `n`, or `n <= 0`, where CPython loops
    // forever).
    let k = match int_value_of(n) {
        Some(value)
            if matches!(
                n.kind(),
                crate::value::ValueKind::Int(_)
                    | crate::value::ValueKind::BigInt(_)
                    | crate::value::ValueKind::Bool(_)
            ) =>
        {
            u64_value(value.bits())
        }
        _ => super::support::call_method(interp, n, "bit_length", &[])?,
    };
    loop {
        let r = match bits {
            BitsSrc::Py(f) => call(interp, f.clone(), std::slice::from_ref(&k))?,
            native => {
                let k = exact_int(&k).unwrap_or_default().max(0) as u64;
                native.bits(interp, this, k)?
            }
        };
        if !compare(interp, &r, BinaryOp::Ge, n)? {
            return Ok(r);
        }
    }
}

fn clone_bits(bits: &BitsSrc) -> BitsSrc {
    match bits {
        BitsSrc::Mt => BitsSrc::Mt,
        BitsSrc::Sys => BitsSrc::Sys,
        BitsSrc::Py(f) => BitsSrc::Py(f.clone()),
    }
}

/// `_randbelow_without_getrandbits(self, n, maxsize)` over a resolved
/// `random`.
pub(super) fn without_getrandbits(
    interp: &mut Interpreter,
    this: &Value,
    src: &RandomSrc,
    n: &Value,
    maxsize: &Value,
) -> Result<Value> {
    if src.is_native()
        && let (Some(small), Some(max)) = (exact_int(n), exact_int(maxsize))
        && small > 0
        && max as u64 == MAXSIZE
        && (small as u64) < MAXSIZE
    {
        return Ok(u64_value(
            Below::Without(match src {
                RandomSrc::Sys => RandomSrc::Sys,
                _ => RandomSrc::Mt,
            })
            .below_native(this, small as u64)?,
        ));
    }
    if compare(interp, n, BinaryOp::Ge, maxsize)? {
        warn(interp, RANDBELOW_WARNING)?;
        let r = src.draw(interp, this)?;
        let scaled = binop(interp, &r, BinaryOp::Mul, n)?;
        return floor(interp, &scaled);
    }
    let rem = binop(interp, maxsize, BinaryOp::Mod, n)?;
    let kept = binop(interp, maxsize, BinaryOp::Sub, &rem)?;
    let limit = binop(interp, &kept, BinaryOp::Div, maxsize)?;
    let mut r = src.draw(interp, this)?;
    while compare(interp, &r, BinaryOp::Ge, &limit)? {
        r = src.draw(interp, this)?;
    }
    let scaled = binop(interp, &r, BinaryOp::Mul, maxsize)?;
    let floored = floor(interp, &scaled)?;
    binop(interp, &floored, BinaryOp::Mod, n)
}

/// `math.floor(value)`.
pub(super) fn floor(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    call(
        interp,
        Value::builtin_function("math.floor"),
        std::slice::from_ref(value),
    )
}
