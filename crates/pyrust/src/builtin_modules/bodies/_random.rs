// `_random` module — 100% native Rust port of CPython 3.12.3's
// `Modules/_randommodule.c` (issue #3494).
//
// `_random.Random` is the MT19937 Mersenne Twister core that `random.Random`
// (`bodies/random.rs`) subclasses: `random()`, `seed([n])`, `getstate()`,
// `setstate(state)` and `getrandbits(k)`.  The generator state (624 words and
// the read index) lives in an invisible native slot of each instance; an
// object created without running `seed()` holds the zero-filled state CPython's
// `PyType_GenericAlloc` gives it.
//
// Seeding is `random_seed` exactly: `None` reads 624 words of OS entropy, an
// `int` uses the 32-bit little-endian limbs of its absolute value, and any
// other object its `hash()` reinterpreted as an unsigned `size_t`; the key is
// fed to the reference `init_by_array`, so every stream is bit-identical to
// CPython's.
//
// Reference: CPython 3.12 `Modules/_randommodule.c`

use std::cell::RefCell;
use std::rc::Rc;

use num_bigint::{BigInt, BigUint};
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyClass, PyModule, Value, ValueKind};

type ClassRc = Rc<RefCell<PyClass>>;

const MODULE_DOC: &str = "Module implements the Mersenne Twister random number generator.";
const RANDOM_DOC: &str =
    "Random() -> create a random number generator with its own internal state.";

/// Degree of recurrence of MT19937 (the state is `N` 32-bit words).
pub(crate) const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

/// `vars(_random.Random)` order in CPython (no `__new__`: pyrust allocates
/// instances through `object.__new__`).
const CLASS_ORDER: &[&str] = &[
    "__init__",
    "random",
    "seed",
    "getstate",
    "setstate",
    "getrandbits",
    "__doc__",
    "__module__",
];

// Per-interpreter state (`gil_local!`):
// (a) Why: the methods check their receiver (CPython's descriptor check) and
//     `__init__` rejects keywords only for objects whose `__init__` is still
//     this class's, so they need the class object without a module reference.
// (b) Every generation is kept (a re-import after `del sys.modules[...]`
//     creates a new class); instances of any of them are accepted.
pyrust_core::gil_local! {
    static CLASSES: RefCell<Vec<ClassRc>> = const { RefCell::new(Vec::new()) };
}

/// The generator state of one `_random.Random` object (`RandomObject`).
pub(crate) struct Mt {
    state: [u32; N],
    index: usize,
}

impl Mt {
    /// The state of an object that was never seeded: `PyType_GenericAlloc`
    /// zero-fills it.
    fn zeroed() -> Self {
        Mt {
            state: [0; N],
            index: 0,
        }
    }

    /// `init_genrand`.
    fn init_genrand(&mut self, s: u32) {
        let mt = &mut self.state;
        mt[0] = s;
        for i in 1..N {
            let prev = mt[i - 1];
            mt[i] = 1_812_433_253_u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = N;
    }

    /// `init_by_array`: seed from a non-empty key of 32-bit words.
    pub(crate) fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let mt = &mut self.state;
        let mut i = 1;
        let mut j = 0;
        for _ in 0..N.max(key.len()) {
            let prev = mt[i - 1];
            mt[i] = (mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = mt[i - 1];
            mt[i] =
                (mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        // MSB is 1, assuring a non-zero initial array.
        mt[0] = 0x8000_0000;
    }

    /// `genrand_uint32`.
    pub(crate) fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            let mt = &mut self.state;
            let mag = |y: u32| if y & 1 == 1 { MATRIX_A } else { 0 };
            for kk in 0..N - M {
                let y = (mt[kk] & UPPER_MASK) | (mt[kk + 1] & LOWER_MASK);
                mt[kk] = mt[kk + M] ^ (y >> 1) ^ mag(y);
            }
            for kk in N - M..N - 1 {
                let y = (mt[kk] & UPPER_MASK) | (mt[kk + 1] & LOWER_MASK);
                mt[kk] = mt[kk + M - N] ^ (y >> 1) ^ mag(y);
            }
            let y = (mt[N - 1] & UPPER_MASK) | (mt[0] & LOWER_MASK);
            mt[N - 1] = mt[M - 1] ^ (y >> 1) ^ mag(y);
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `_random.Random.random()`: a 53-bit float in `[0.0, 1.0)`.
    pub(crate) fn random(&mut self) -> f64 {
        let a = self.next_u32() >> 5;
        let b = self.next_u32() >> 6;
        (f64::from(a) * 67_108_864.0 + f64::from(b)) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `getrandbits(k)` for `0 < k <= 64`.
    pub(crate) fn bits_u64(&mut self, k: u32) -> u64 {
        if k <= 32 {
            return u64::from(self.next_u32() >> (32 - k));
        }
        let low = u64::from(self.next_u32());
        let high = u64::from(self.next_u32() >> (64 - k));
        low | (high << 32)
    }

    /// The words of `getrandbits(k)` (`k > 0`), least significant first: every
    /// word is one draw, the last one keeping only its `k % 32` top bits.
    pub(crate) fn bits_words(&mut self, k: u64) -> Vec<u32> {
        let words = ((k - 1) / 32 + 1) as usize;
        let mut out = Vec::with_capacity(words);
        let mut left = k;
        for _ in 0..words {
            let mut r = self.next_u32();
            if left < 32 {
                r >>= 32 - left;
            }
            out.push(r);
            left = left.saturating_sub(32);
        }
        out
    }

    /// `getrandbits(k)` for `k >= 0` as a Python `int`.
    pub(crate) fn getrandbits(&mut self, k: u64) -> Value {
        match k {
            0 => Value::int(0),
            1..=64 => u64_value(self.bits_u64(k as u32)),
            _ => pyrust_core::value_from_bigint(BigInt::from(BigUint::new(self.bits_words(k)))),
        }
    }

    /// `randbytes(n)` (`getrandbits(n * 8).to_bytes(n, 'little')`).
    pub(crate) fn randbytes(&mut self, n: usize) -> Vec<u8> {
        if n == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(n);
        for word in self.bits_words(n as u64 * 8) {
            let take = (n - out.len()).min(4);
            out.extend_from_slice(&word.to_le_bytes()[..take]);
        }
        out
    }
}

/// A Python `int` from a `u64`.
pub(crate) fn u64_value(n: u64) -> Value {
    match i64::try_from(n) {
        Ok(small) => Value::int(small),
        Err(_) => pyrust_core::value_from_bigint(BigInt::from(n)),
    }
}

/// Run `f` on the generator state of `this`, creating the zero-filled state of
/// a never-seeded object first.
pub(crate) fn with_mt<R>(this: &Value, f: impl FnOnce(&mut Mt) -> R) -> Result<R> {
    if !has_native_state::<Mt>(this) {
        set_native_state(this, Mt::zeroed())?;
    }
    with_native_state_mut(this, "_random.Random", |mt: &mut Mt| Ok(f(mt)))
}

/// `n` bytes from the OS cryptographically secure source (`os.urandom`).
pub(crate) fn urandom(n: usize) -> Result<Vec<u8>> {
    let mut buffer = vec![0_u8; n];
    if n == 0 {
        return Ok(buffer);
    }
    getrandom::getrandom(&mut buffer).map_err(|error| match error.raw_os_error() {
        Some(code) => PyError::from_io_error(&std::io::Error::from_raw_os_error(code), None),
        None => PyError::named("OSError", error.to_string()),
    })?;
    Ok(buffer)
}

/// The `init_by_array` key of a non-negative integer: its 32-bit limbs, least
/// significant first (`[0]` for zero).
fn key_of(magnitude: &BigUint) -> Vec<u32> {
    let digits = magnitude.to_u32_digits();
    if digits.is_empty() { vec![0] } else { digits }
}

/// `random_seed_urandom`, falling back on `random_seed_time_pid` when the OS
/// source fails.
fn entropy_key() -> Vec<u32> {
    if let Ok(bytes) = urandom(N * 4) {
        return bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_ne_bytes(*c))
            .collect();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let pid = std::process::id();
    let mono = std::time::Instant::now().elapsed().as_nanos() as u64;
    vec![
        now as u32,
        (now >> 32) as u32,
        pid,
        mono as u32,
        (mono >> 32) as u32,
    ]
}

/// The magnitude of an `int` (or `int` subclass) value.
pub(crate) fn int_magnitude(value: &Value) -> Option<BigUint> {
    match value.kind() {
        ValueKind::Int(n) => Some(BigUint::from(n.unsigned_abs())),
        ValueKind::Bool(b) => Some(BigUint::from(u8::from(b))),
        ValueKind::BigInt(n) => Some(n.magnitude().clone()),
        ValueKind::PyInstance(_) => {
            crate::interpreter::builtin_data_backing(value).and_then(|backing| {
                match backing.kind() {
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                        int_magnitude(&backing)
                    }
                    _ => None,
                }
            })
        }
        _ => None,
    }
}

/// `random_seed(self, arg)`.
pub(crate) fn seed_object(interp: &mut Interpreter, this: &Value, arg: &Value) -> Result<()> {
    let key = if arg.is_none() {
        entropy_key()
    } else if let Some(magnitude) = int_magnitude(arg) {
        key_of(&magnitude)
    } else {
        let hash = interp.call_function_expanded(
            Value::builtin_function("hash"),
            &[ExpandedCallArg {
                name: None,
                value: arg.clone(),
            }],
        )?;
        let hash = hash.as_int().unwrap_or_default();
        key_of(&BigUint::from(hash as u64))
    };
    with_mt(this, |mt| mt.init_by_array(&key))
}

/// `random_getstate`: the 624 state words and the read index.
pub(crate) fn getstate_of(this: &Value) -> Result<Value> {
    with_mt(this, |mt| {
        let mut items: Vec<Value> = mt.state.iter().map(|w| Value::int(i64::from(*w))).collect();
        items.push(Value::int(mt.index as i64));
        Value::tuple(items)
    })
}

/// Whether a `u64` fits a C `unsigned long` (32 bits on Windows).
#[cfg(windows)]
fn fits_c_ulong(value: u64) -> bool {
    u32::try_from(value).is_ok()
}

#[cfg(not(windows))]
fn fits_c_ulong(_value: u64) -> bool {
    true
}

/// Whether an `i64` fits a C `long` (32 bits on Windows).
#[cfg(windows)]
fn fits_c_long(value: i64) -> bool {
    i32::try_from(value).is_ok()
}

#[cfg(not(windows))]
fn fits_c_long(_value: i64) -> bool {
    true
}

/// `PyLong_AsUnsignedLong` truncated to the 32-bit state word.
fn state_word(value: &Value) -> Result<u32> {
    let backing;
    let value = match value.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => value,
        _ => match crate::interpreter::builtin_data_backing(value) {
            Some(b)
                if matches!(
                    b.kind(),
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                ) =>
            {
                backing = b;
                &backing
            }
            _ => return Err(type_error("an integer is required")),
        },
    };
    let n: BigInt = match value.kind() {
        ValueKind::Int(n) => BigInt::from(n),
        ValueKind::Bool(b) => BigInt::from(u8::from(b)),
        ValueKind::BigInt(n) => n.clone(),
        _ => unreachable!("checked above"),
    };
    if n.sign() == num_bigint::Sign::Minus {
        return Err(PyError::named(
            "OverflowError",
            "can't convert negative value to unsigned int",
        ));
    }
    match u64::try_from(&n) {
        Ok(word) if fits_c_ulong(word) => Ok(word as u32),
        _ => Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C unsigned long",
        )),
    }
}

/// `random_setstate`.
pub(crate) fn setstate_of(interp: &mut Interpreter, this: &Value, state: &Value) -> Result<()> {
    let items: Vec<Value> = match state.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => match crate::interpreter::builtin_data_backing(state) {
            Some(backing) if matches!(backing.kind(), ValueKind::Tuple(_)) => backing
                .as_tuple()
                .map(<[Value]>::to_vec)
                .unwrap_or_default(),
            _ => return Err(type_error("state vector must be a tuple")),
        },
    };
    if items.len() != N + 1 {
        return Err(value_error("state vector is the wrong size"));
    }
    let mut words = [0_u32; N];
    for (word, item) in words.iter_mut().zip(&items) {
        *word = state_word(item)?;
    }
    let index = interp.value_to_index(&items[N], |v| {
        type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            pyrust_core::builtin_type_name(v)
        ))
    })?;
    let index = index_i64(&index)
        .filter(|i| fits_c_long(*i))
        .ok_or_else(|| {
            PyError::named("OverflowError", "Python int too large to convert to C long")
        })?;
    if !(0..=N as i64).contains(&index) {
        return Err(value_error("invalid state"));
    }
    with_mt(this, |mt| {
        mt.state = words;
        mt.index = index as usize;
    })
}

fn index_i64(value: &Value) -> Option<i64> {
    match value.kind() {
        ValueKind::Int(n) => Some(n),
        ValueKind::Bool(b) => Some(i64::from(b)),
        _ => None,
    }
}

/// `_PyLong_AsInt(k)` for `getrandbits(k)`, then the non-negative check.
pub(crate) fn getrandbits_arg(interp: &mut Interpreter, k: &Value) -> Result<u64> {
    let index = interp.value_to_index(k, |v| {
        type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            pyrust_core::builtin_type_name(v)
        ))
    })?;
    let k = index_i64(&index)
        .filter(|k| i32::try_from(*k).is_ok())
        .ok_or_else(|| {
            PyError::named("OverflowError", "Python int too large to convert to C int")
        })?;
    if k < 0 {
        return Err(value_error("number of bits must be non-negative"));
    }
    Ok(k as u64)
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

/// Whether `value` is an instance of (any generation of) `_random.Random`.
pub(crate) fn is_random_instance(value: &Value) -> bool {
    let Some(instance) = value.as_py_instance_rc() else {
        return false;
    };
    let class = Rc::clone(&instance.borrow().class);
    CLASSES.with(|classes| {
        classes
            .borrow()
            .iter()
            .any(|base| crate::interpreter::class_is_subclass_of(&class, base))
    })
}

/// The calling convention of a `_random.Random` C method.
#[derive(Clone, Copy)]
enum Flavor {
    /// `METH_NOARGS`.
    NoArgs,
    /// `METH_O`.
    One,
    /// `seed([n])` (`METH_FASTCALL`, at most one positional argument).
    UpToOne,
}

/// Check a C method call the way `method_vectorcall_*` does (`self`, the
/// descriptor check, no keywords, the positional count) and return `self` and
/// the remaining positional arguments.
fn c_method<'a>(
    args: &'a [ExpandedCallArg],
    name: &str,
    flavor: Flavor,
) -> Result<(&'a Value, Vec<&'a Value>)> {
    let mut positional = args.iter().filter(|a| a.name.is_none()).map(|a| &a.value);
    let Some(this) = positional.next() else {
        return Err(type_error(format!(
            "unbound method Random.{name}() needs an argument"
        )));
    };
    if !is_random_instance(this) {
        return Err(type_error(format!(
            "descriptor '{name}' for '_random.Random' objects doesn't apply to a '{}' object",
            value_type_name_str(this)
        )));
    }
    if args.iter().any(|a| a.name.is_some()) {
        return Err(type_error(format!(
            "Random.{name}() takes no keyword arguments"
        )));
    }
    let rest: Vec<&Value> = positional.collect();
    match flavor {
        Flavor::NoArgs if !rest.is_empty() => Err(type_error(format!(
            "Random.{name}() takes no arguments ({} given)",
            rest.len()
        ))),
        Flavor::One if rest.len() != 1 => Err(type_error(format!(
            "Random.{name}() takes exactly one argument ({} given)",
            rest.len()
        ))),
        Flavor::UpToOne if rest.len() > 1 => Err(type_error(format!(
            "{name} expected at most 1 argument, got {}",
            rest.len()
        ))),
        _ => Ok((this, rest)),
    }
}

/// `random_init`: `_random.Random([x])` seeds like `seed(x)`.
fn init(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<()> {
    let mut positional = args.iter().filter(|a| a.name.is_none()).map(|a| &a.value);
    let Some(this) = positional.next() else {
        return Err(type_error(
            "descriptor '__init__' of '_random.Random' object needs an argument",
        ));
    };
    if !is_random_instance(this) {
        return Err(type_error(format!(
            "descriptor '__init__' requires a '_random.Random' object but received a '{}'",
            value_type_name_str(this)
        )));
    }
    if args.iter().any(|a| a.name.is_some()) && uses_own_init(this) {
        return Err(type_error("Random() takes no keyword arguments"));
    }
    let rest: Vec<&Value> = positional.collect();
    if rest.len() > 1 {
        return Err(type_error("Random() requires 0 or 1 argument"));
    }
    let arg = rest.first().map_or_else(Value::none, |v| (*v).clone());
    seed_object(interp, this, &arg)
}

/// Whether `type(this).__init__` is still `_random.Random.__init__`.
fn uses_own_init(this: &Value) -> bool {
    let Some(instance) = this.as_py_instance_rc() else {
        return false;
    };
    let class = Rc::clone(&instance.borrow().class);
    let Some(owner) = crate::interpreter::lookup_class_attr_owner(&class, "__init__") else {
        return false;
    };
    CLASSES.with(|classes| classes.borrow().iter().any(|c| Rc::ptr_eq(c, &owner)))
}

/// Register the class and restore CPython's `vars(_random.Random)` order.
pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let class = module.borrow().get_attr_value("Random");
    let Some(ValueKind::PyClass(class)) = class.as_ref().map(Value::kind) else {
        return Err(PyError::runtime("_random: missing Random".to_string()));
    };
    {
        let mut class = class.borrow_mut();
        for name in CLASS_ORDER {
            if let Some(value) = class.attrs.shift_remove(*name) {
                class.attrs.insert((*name).to_string(), value);
            }
        }
        class.bump_mutation_version();
    }
    CLASSES.with(|classes| classes.borrow_mut().push(Rc::clone(class)));
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
    }

    class Random {
        constants {
            "__doc__" => Value::string(RANDOM_DOC),
        }

        fn __init__(args) -> Result<Value> {
            init(_interp, args)?;
            Ok(Value::none())
        }

        fn random(args) -> Result<Value> {
            let (this, _) = c_method(args, "random", Flavor::NoArgs)?;
            with_mt(this, |mt| Value::float(mt.random()))
        }

        fn seed(args) -> Result<Value> {
            let (this, rest) = c_method(args, "seed", Flavor::UpToOne)?;
            let arg = rest.first().map_or_else(Value::none, |v| (*v).clone());
            seed_object(_interp, this, &arg)?;
            Ok(Value::none())
        }

        fn getstate(args) -> Result<Value> {
            let (this, _) = c_method(args, "getstate", Flavor::NoArgs)?;
            getstate_of(this)
        }

        fn setstate(args) -> Result<Value> {
            let (this, rest) = c_method(args, "setstate", Flavor::One)?;
            setstate_of(_interp, this, rest[0])?;
            Ok(Value::none())
        }

        fn getrandbits(args) -> Result<Value> {
            let (this, rest) = c_method(args, "getrandbits", Flavor::One)?;
            let k = getrandbits_arg(_interp, rest[0])?;
            with_mt(this, |mt| mt.getrandbits(k))
        }
    }
}
