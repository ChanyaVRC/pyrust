// `random` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/random.py` (issues #2785, #3494); there is no Python-source injection.
//
// `Random` subclasses the MT19937 core `_random.Random` (`bodies/_random.rs`)
// and ports every method of the CPython class: seeding (`version=2` folds a
// `str` / `bytes` / `bytearray` seed through SHA-512 exactly like CPython, so
// every seed type gives CPython's stream), `getstate` / `setstate` /
// pickling, `randbytes`, `randrange`, `randint`, `choice`, `shuffle`,
// `sample`, `choices`, `binomialvariate` and the continuous distributions.
// `SystemRandom` draws from `os.urandom`.  The module-level functions are the
// bound methods of the module's `_inst = Random()`.
//
//  * `random/dispatch.rs` — `self.random` / `self.getrandbits` /
//    `self._randbelow` resolution: like the Python source, every method draws
//    through `self`, so subclasses overriding `random()` or `getrandbits()`
//    (and `__init_subclass__`'s `_randbelow` choice) behave as in CPython;
//    when the lookup finds this module's own method it runs natively.
//  * `random/arith.rs`, `random/dists.rs` — the distributions, written once
//    over native floats or Python objects (`Fraction`, `Decimal`, ...).
//  * `random/seq.rs` — the integer, bytes and sequence methods.
//  * `random/methods.rs` — seeding, state, the distribution entry points and
//    `SystemRandom`'s generator methods.
//
// Deliberate differences: native functions have no `__doc__`, the module
// constants (`NV_MAGICCONST`, `TWOPI`, ...) are read when a distribution
// starts rather than at every use, and `_sha512`, `_test_generator` and
// `_test` are not bound.
//
// Reference: <https://docs.python.org/3.12/library/random.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyClass, PyModule, Value, ValueKind};

#[path = "random/arith.rs"]
mod arith;
#[path = "random/dispatch.rs"]
mod dispatch;
#[path = "random/dists.rs"]
mod dists;
#[path = "random/docs.rs"]
mod docs;
#[path = "random/methods.rs"]
mod methods;
#[path = "random/seq.rs"]
mod seq;
#[path = "random/support.rs"]
mod support;

use dispatch::{NativeId, RECIP_BPF, register};

type ClassRc = Rc<RefCell<PyClass>>;

/// `__all__`, in CPython's order.
const ALL: [&str; 26] = [
    "Random",
    "SystemRandom",
    "betavariate",
    "binomialvariate",
    "choice",
    "choices",
    "expovariate",
    "gammavariate",
    "gauss",
    "getrandbits",
    "getstate",
    "lognormvariate",
    "normalvariate",
    "paretovariate",
    "randbytes",
    "randint",
    "random",
    "randrange",
    "sample",
    "seed",
    "setstate",
    "shuffle",
    "triangular",
    "uniform",
    "vonmisesvariate",
    "weibullvariate",
];

/// The module-level functions: bound methods of `_inst`, in CPython's order.
const INST_METHODS: [&str; 24] = [
    "seed",
    "random",
    "uniform",
    "triangular",
    "randint",
    "choice",
    "randrange",
    "sample",
    "shuffle",
    "choices",
    "normalvariate",
    "lognormvariate",
    "expovariate",
    "vonmisesvariate",
    "gammavariate",
    "gauss",
    "betavariate",
    "binomialvariate",
    "paretovariate",
    "weibullvariate",
    "getstate",
    "setstate",
    "getrandbits",
    "randbytes",
];

/// The private names `Lib/random.py` imports, as `(alias, module, name)`.
const IMPORTED: [(&str, &str, &str); 22] = [
    ("_warn", "warnings", "warn"),
    ("_log", "math", "log"),
    ("_exp", "math", "exp"),
    ("_pi", "math", "pi"),
    ("_e", "math", "e"),
    ("_ceil", "math", "ceil"),
    ("_sqrt", "math", "sqrt"),
    ("_acos", "math", "acos"),
    ("_cos", "math", "cos"),
    ("_sin", "math", "sin"),
    ("_floor", "math", "floor"),
    ("_isfinite", "math", "isfinite"),
    ("_lgamma", "math", "lgamma"),
    ("_fabs", "math", "fabs"),
    ("_log2", "math", "log2"),
    ("_urandom", "os", "urandom"),
    ("_Sequence", "_collections_abc", "Sequence"),
    ("_index", "operator", "index"),
    ("_accumulate", "itertools", "accumulate"),
    ("_repeat", "itertools", "repeat"),
    ("_bisect", "bisect", "bisect"),
    ("_os", "os", ""),
];

/// CPython's `vars(random)` order for the names this module binds.
const MODULE_ORDER: &[&str] = &[
    "_warn",
    "_log",
    "_exp",
    "_pi",
    "_e",
    "_ceil",
    "_sqrt",
    "_acos",
    "_cos",
    "_sin",
    "TWOPI",
    "_floor",
    "_isfinite",
    "_lgamma",
    "_fabs",
    "_log2",
    "_urandom",
    "_Sequence",
    "_index",
    "_accumulate",
    "_repeat",
    "_bisect",
    "_os",
    "_random",
    "__all__",
    "NV_MAGICCONST",
    "LOG4",
    "SG_MAGICCONST",
    "BPF",
    "RECIP_BPF",
    "_ONE",
    "Random",
    "SystemRandom",
    "_inst",
];

/// CPython's `vars(random.Random)` order.
const RANDOM_ORDER: &[&str] = &[
    "__module__",
    "__doc__",
    "VERSION",
    "__init__",
    "seed",
    "getstate",
    "setstate",
    "__getstate__",
    "__setstate__",
    "__reduce__",
    "__init_subclass__",
    "_randbelow_with_getrandbits",
    "_randbelow_without_getrandbits",
    "_randbelow",
    "randbytes",
    "randrange",
    "randint",
    "choice",
    "shuffle",
    "sample",
    "choices",
    "uniform",
    "triangular",
    "normalvariate",
    "gauss",
    "lognormvariate",
    "expovariate",
    "vonmisesvariate",
    "gammavariate",
    "betavariate",
    "paretovariate",
    "weibullvariate",
    "binomialvariate",
];

/// CPython's `vars(random.SystemRandom)` order.
const SYSTEM_ORDER: &[&str] = &[
    "__module__",
    "__doc__",
    "random",
    "getrandbits",
    "randbytes",
    "seed",
    "_notimplemented",
    "getstate",
    "setstate",
    "_randbelow",
];

const fn sig(
    qualname: &'static str,
    params: &'static [&'static str],
    defaults: usize,
    kwonly: &'static [&'static str],
) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly,
        var_kw: false,
    }
}

// The `def`s of `Lib/random.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording.
const SIG_INIT: Sig = sig("Random.__init__", &["self", "x"], 1, &[]);
const SIG_SEED: Sig = sig("Random.seed", &["self", "a", "version"], 2, &[]);
const SIG_GETSTATE: Sig = sig("Random.getstate", &["self"], 0, &[]);
const SIG_SETSTATE: Sig = sig("Random.setstate", &["self", "state"], 0, &[]);
const SIG_DUNDER_GETSTATE: Sig = sig("Random.__getstate__", &["self"], 0, &[]);
const SIG_DUNDER_SETSTATE: Sig = sig("Random.__setstate__", &["self", "state"], 0, &[]);
const SIG_REDUCE: Sig = sig("Random.__reduce__", &["self"], 0, &[]);
const SIG_INIT_SUBCLASS: Sig = Sig {
    qualname: "Random.__init_subclass__",
    params: &["cls"],
    defaults: 0,
    kwonly: &[],
    var_kw: true,
};
const SIG_WITH: Sig = sig("Random._randbelow_with_getrandbits", &["self", "n"], 0, &[]);
const SIG_WITHOUT: Sig = sig(
    "Random._randbelow_without_getrandbits",
    &["self", "n", "maxsize"],
    1,
    &[],
);
const SIG_RANDBYTES: Sig = sig("Random.randbytes", &["self", "n"], 0, &[]);
const SIG_RANDRANGE: Sig = sig(
    "Random.randrange",
    &["self", "start", "stop", "step"],
    2,
    &[],
);
const SIG_RANDINT: Sig = sig("Random.randint", &["self", "a", "b"], 0, &[]);
const SIG_CHOICE: Sig = sig("Random.choice", &["self", "seq"], 0, &[]);
const SIG_SHUFFLE: Sig = sig("Random.shuffle", &["self", "x"], 0, &[]);
const SIG_SAMPLE: Sig = sig(
    "Random.sample",
    &["self", "population", "k"],
    0,
    &["counts"],
);
const SIG_CHOICES: Sig = sig(
    "Random.choices",
    &["self", "population", "weights"],
    1,
    &["cum_weights", "k"],
);
const SIG_UNIFORM: Sig = sig("Random.uniform", &["self", "a", "b"], 0, &[]);
const SIG_TRIANGULAR: Sig = sig(
    "Random.triangular",
    &["self", "low", "high", "mode"],
    3,
    &[],
);
const SIG_NORMALVARIATE: Sig = sig("Random.normalvariate", &["self", "mu", "sigma"], 2, &[]);
const SIG_GAUSS: Sig = sig("Random.gauss", &["self", "mu", "sigma"], 2, &[]);
const SIG_LOGNORMVARIATE: Sig = sig("Random.lognormvariate", &["self", "mu", "sigma"], 0, &[]);
const SIG_EXPOVARIATE: Sig = sig("Random.expovariate", &["self", "lambd"], 1, &[]);
const SIG_VONMISESVARIATE: Sig = sig("Random.vonmisesvariate", &["self", "mu", "kappa"], 0, &[]);
const SIG_GAMMAVARIATE: Sig = sig("Random.gammavariate", &["self", "alpha", "beta"], 0, &[]);
const SIG_BETAVARIATE: Sig = sig("Random.betavariate", &["self", "alpha", "beta"], 0, &[]);
const SIG_PARETOVARIATE: Sig = sig("Random.paretovariate", &["self", "alpha"], 0, &[]);
const SIG_WEIBULLVARIATE: Sig = sig("Random.weibullvariate", &["self", "alpha", "beta"], 0, &[]);
const SIG_BINOMIALVARIATE: Sig = sig("Random.binomialvariate", &["self", "n", "p"], 2, &[]);
const SIG_SYS_RANDOM: Sig = sig("SystemRandom.random", &["self"], 0, &[]);
const SIG_SYS_GETRANDBITS: Sig = sig("SystemRandom.getrandbits", &["self", "k"], 0, &[]);
const SIG_SYS_RANDBYTES: Sig = sig("SystemRandom.randbytes", &["self", "n"], 0, &[]);

/// `def f(self, *args, **kwds)`: `self` is the only required argument.
fn require_self(qualname: &str, args: &[ExpandedCallArg]) -> Result<()> {
    let has_self = args
        .iter()
        .any(|a| a.name.is_none() || a.name.as_deref() == Some("self"));
    if has_self {
        Ok(())
    } else {
        Err(support::type_error(format!(
            "{qualname}() missing 1 required positional argument: 'self'"
        )))
    }
}

fn all_list() -> Value {
    Value::list(ALL.iter().map(Value::string).collect())
}

fn reorder<S: std::hash::BuildHasher>(
    attrs: &mut indexmap::IndexMap<String, Value, S>,
    order: &[&str],
) {
    for name in order {
        if let Some(value) = attrs.shift_remove(*name) {
            attrs.insert((*name).to_string(), value);
        }
    }
}

// Per-interpreter state (`gil_local!`):
// (a) Why: `_inst`, the default generator every module-level function is
//     bound to, belongs to the interpreter (GIL group) that imported `random`
//     — never to a process-wide static or a raw `thread_local!` — and it
//     stays owned for that interpreter's lifetime.
// (b) Every generation is kept: a re-import after `del sys.modules[...]`
//     creates a new `_inst` (as CPython's fresh module body does) while the
//     functions bound to an older one keep working.
pyrust_core::gil_local! {
    static DEFAULT_INSTANCES: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

/// `_inst = Random()`: create and record this interpreter's default
/// generator.
fn new_default_instance(interp: &mut Interpreter, random_cls: &ClassRc) -> Result<Value> {
    let inst = support::call(interp, Value::py_class(Rc::clone(random_cls)), &[])?;
    DEFAULT_INSTANCES.with(|all| all.borrow_mut().push(inst.clone()));
    Ok(inst)
}

fn module_class(module: &Rc<RefCell<PyModule>>, name: &str) -> Result<ClassRc> {
    match module
        .borrow()
        .get_attr_value(name)
        .as_ref()
        .map(Value::kind)
    {
        Some(ValueKind::PyClass(class)) => Ok(Rc::clone(class)),
        _ => Err(PyError::runtime(format!("random: missing {name}"))),
    }
}

fn class_attr(class: &ClassRc, name: &str) -> Result<Value> {
    class
        .borrow()
        .attrs
        .get(name)
        .cloned()
        .ok_or_else(|| PyError::runtime(format!("random: missing {name}")))
}

/// Make `Random` a subclass of `_random.Random`, add the class attributes
/// the CPython class body creates by assignment (`_randbelow`,
/// `getstate = setstate = _notimplemented`, and the `_randbelow` that
/// `__init_subclass__` gives `SystemRandom`), register the native methods
/// for `self.<method>` resolution, and create `_inst` and the module-level
/// functions.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let core_module = interp.load_module("_random")?;
    let base = match interp.get_attr(&core_module, "Random")?.kind() {
        ValueKind::PyClass(class) => Rc::clone(class),
        _ => {
            return Err(PyError::runtime(
                "random: _random.Random is not a class".to_string(),
            ));
        }
    };
    let random_cls = module_class(module, "Random")?;
    let system_cls = module_class(module, "SystemRandom")?;
    link_class_bases(&random_cls, vec![Rc::clone(&base)]);

    let with = class_attr(&random_cls, "_randbelow_with_getrandbits")?;
    {
        let mut class = random_cls.borrow_mut();
        class.attrs.insert("_randbelow".to_string(), with.clone());
        reorder(&mut class.attrs, RANDOM_ORDER);
        class.bump_mutation_version();
    }
    let notimplemented = class_attr(&system_cls, "_notimplemented")?;
    {
        let mut class = system_cls.borrow_mut();
        class
            .attrs
            .insert("getstate".to_string(), notimplemented.clone());
        class.attrs.insert("setstate".to_string(), notimplemented);
        class.attrs.insert("_randbelow".to_string(), with.clone());
        reorder(&mut class.attrs, SYSTEM_ORDER);
        class.bump_mutation_version();
    }

    register(class_attr(&base, "random")?, NativeId::MtRandom);
    register(class_attr(&base, "getrandbits")?, NativeId::MtGetrandbits);
    register(class_attr(&system_cls, "random")?, NativeId::SysRandom);
    register(
        class_attr(&system_cls, "getrandbits")?,
        NativeId::SysGetrandbits,
    );
    register(with, NativeId::RandbelowWith);
    for (name, id) in [
        ("_randbelow_without_getrandbits", NativeId::RandbelowWithout),
        ("randrange", NativeId::Randrange),
        ("normalvariate", NativeId::Normalvariate),
        ("gammavariate", NativeId::Gammavariate),
        ("binomialvariate", NativeId::Binomialvariate),
    ] {
        register(class_attr(&random_cls, name)?, id);
    }

    let inst = new_default_instance(interp, &random_cls)?;
    let mut bound = Vec::with_capacity(INST_METHODS.len());
    for name in INST_METHODS {
        bound.push((name, interp.get_attr(&inst, name)?));
    }
    let mut imported = Vec::with_capacity(IMPORTED.len() + 1);
    for (alias, source, name) in IMPORTED {
        let source = interp.load_module(source)?;
        let value = if name.is_empty() {
            source
        } else {
            interp.get_attr(&source, name)?
        };
        imported.push((alias, value));
    }
    imported.push(("_random", core_module));

    let mut module = module.borrow_mut();
    for (alias, value) in imported {
        module.insert_attr(alias.to_string(), value);
    }
    module.insert_attr("_inst".to_string(), inst);
    for (name, value) in bound {
        module.insert_attr(name.to_string(), value);
    }
    let mut order: Vec<&str> = MODULE_ORDER.to_vec();
    order.extend(INST_METHODS);
    reorder(&mut module.attrs, &order);
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_list(),
        "TWOPI" => Value::float(std::f64::consts::TAU),
        "NV_MAGICCONST" => Value::float(dists::consts().nv_magicconst),
        "LOG4" => Value::float(dists::consts().log4),
        "SG_MAGICCONST" => Value::float(dists::consts().sg_magicconst),
        "BPF" => Value::int(53),
        "RECIP_BPF" => Value::float(RECIP_BPF),
        "_ONE" => Value::int(1),
    }

    class Random {
        constants {
            "__doc__" => Value::string(docs::RANDOM_DOC),
            "VERSION" => Value::int(3),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_INIT, args)?;
            let this = b.take_or_none(0);
            let x = b.take_or_none(1);
            support::call_method(_interp, &this, "seed", &[x])?;
            _interp.assign_attr(this, "gauss_next", Value::none())?;
            Ok(Value::none())
        }

        fn seed(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SEED, args)?;
            let this = b.take_or_none(0);
            let a = b.take_or_none(1);
            let version = b.take_or(2, || Value::int(2));
            methods::seed(_interp, &this, a, &version)?;
            Ok(Value::none())
        }

        fn getstate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_GETSTATE, args)?;
            let this = b.take_or_none(0);
            methods::getstate(_interp, &this)
        }

        fn setstate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SETSTATE, args)?;
            let this = b.take_or_none(0);
            let state = b.take_or_none(1);
            methods::setstate(_interp, &this, &state)?;
            Ok(Value::none())
        }

        fn __getstate__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_DUNDER_GETSTATE, args)?;
            let this = b.take_or_none(0);
            support::call_method(_interp, &this, "getstate", &[])
        }

        fn __setstate__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_DUNDER_SETSTATE, args)?;
            let this = b.take_or_none(0);
            let state = b.take_or_none(1);
            support::call_method(_interp, &this, "setstate", &[state])?;
            Ok(Value::none())
        }

        fn __reduce__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_REDUCE, args)?;
            let this = b.take_or_none(0);
            let class = _interp.get_attr(&this, "__class__")?;
            let state = support::call_method(_interp, &this, "getstate", &[])?;
            Ok(Value::tuple(vec![class, Value::tuple(Vec::new()), state]))
        }

        #[classmethod]
        fn __init_subclass__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_INIT_SUBCLASS, args)?;
            let cls = b.take_or_none(0);
            methods::init_subclass(_interp, &cls)?;
            Ok(Value::none())
        }

        fn _randbelow_with_getrandbits(args) -> Result<Value> {
            let mut b = args::bind(&SIG_WITH, args)?;
            let this = b.take_or_none(0);
            let n = b.take_or_none(1);
            let bits = dispatch::bits_src(_interp, &this)?;
            dispatch::with_getrandbits(_interp, &this, &bits, &n)
        }

        fn _randbelow_without_getrandbits(args) -> Result<Value> {
            let mut b = args::bind(&SIG_WITHOUT, args)?;
            let this = b.take_or_none(0);
            let n = b.take_or_none(1);
            let maxsize = b.take_or(2, || Value::int(dispatch::MAXSIZE as i64));
            let src = dispatch::random_src(_interp, &this)?;
            dispatch::without_getrandbits(_interp, &this, &src, &n, &maxsize)
        }

        fn randbytes(args) -> Result<Value> {
            let mut b = args::bind(&SIG_RANDBYTES, args)?;
            let this = b.take_or_none(0);
            let n = b.take_or_none(1);
            seq::randbytes(_interp, &this, &n)
        }

        fn randrange(args) -> Result<Value> {
            let mut b = args::bind(&SIG_RANDRANGE, args)?;
            let this = b.take_or_none(0);
            let start = b.take_or_none(1);
            let stop = b.take(2);
            let step = b.take(3);
            seq::randrange(_interp, &this, &start, stop.as_ref(), step.as_ref())
        }

        fn randint(args) -> Result<Value> {
            let mut b = args::bind(&SIG_RANDINT, args)?;
            let this = b.take_or_none(0);
            let lo = b.take_or_none(1);
            let hi = b.take_or_none(2);
            seq::randint(_interp, &this, &lo, &hi)
        }

        fn choice(args) -> Result<Value> {
            let mut b = args::bind(&SIG_CHOICE, args)?;
            let this = b.take_or_none(0);
            let population = b.take_or_none(1);
            seq::choice(_interp, &this, &population)
        }

        fn shuffle(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SHUFFLE, args)?;
            let this = b.take_or_none(0);
            let x = b.take_or_none(1);
            seq::shuffle(_interp, &this, &x)?;
            Ok(Value::none())
        }

        fn sample(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SAMPLE, args)?;
            let this = b.take_or_none(0);
            let population = b.take_or_none(1);
            let k = b.take_or_none(2);
            let counts = b.take(3);
            seq::sample(_interp, &this, &population, &k, counts.as_ref())
        }

        fn choices(args) -> Result<Value> {
            let mut b = args::bind(&SIG_CHOICES, args)?;
            let this = b.take_or_none(0);
            let population = b.take_or_none(1);
            let weights = b.take(2);
            let cum_weights = b.take(3);
            let k = b.take_or(4, || Value::int(1));
            seq::choices(_interp, &this, &population, weights.as_ref(), cum_weights.as_ref(), &k)
        }

        fn uniform(args) -> Result<Value> {
            let mut b = args::bind(&SIG_UNIFORM, args)?;
            let this = b.take_or_none(0);
            let lo = b.take_or_none(1);
            let hi = b.take_or_none(2);
            methods::uniform(_interp, &this, &lo, &hi)
        }

        fn triangular(args) -> Result<Value> {
            let mut b = args::bind(&SIG_TRIANGULAR, args)?;
            let this = b.take_or_none(0);
            let low = b.take_or(1, || Value::float(0.0));
            let high = b.take_or(2, || Value::float(1.0));
            let mode = b.take_or_none(3);
            methods::triangular(_interp, &this, &low, &high, &mode)
        }

        fn normalvariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NORMALVARIATE, args)?;
            let this = b.take_or_none(0);
            let mu = b.take_or(1, || Value::float(0.0));
            let sigma = b.take_or(2, || Value::float(1.0));
            methods::normalvariate(_interp, &this, &mu, &sigma)
        }

        fn gauss(args) -> Result<Value> {
            let mut b = args::bind(&SIG_GAUSS, args)?;
            let this = b.take_or_none(0);
            let mu = b.take_or(1, || Value::float(0.0));
            let sigma = b.take_or(2, || Value::float(1.0));
            methods::gauss(_interp, &this, &mu, &sigma)
        }

        fn lognormvariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_LOGNORMVARIATE, args)?;
            let this = b.take_or_none(0);
            let mu = b.take_or_none(1);
            let sigma = b.take_or_none(2);
            methods::lognormvariate(_interp, &this, &mu, &sigma)
        }

        fn expovariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXPOVARIATE, args)?;
            let this = b.take_or_none(0);
            let lambd = b.take_or(1, || Value::float(1.0));
            methods::expovariate(_interp, &this, &lambd)
        }

        fn vonmisesvariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_VONMISESVARIATE, args)?;
            let this = b.take_or_none(0);
            let mu = b.take_or_none(1);
            let kappa = b.take_or_none(2);
            methods::vonmisesvariate(_interp, &this, &mu, &kappa)
        }

        fn gammavariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_GAMMAVARIATE, args)?;
            let this = b.take_or_none(0);
            let alpha = b.take_or_none(1);
            let beta = b.take_or_none(2);
            methods::gammavariate(_interp, &this, &alpha, &beta)
        }

        fn betavariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BETAVARIATE, args)?;
            let this = b.take_or_none(0);
            let alpha = b.take_or_none(1);
            let beta = b.take_or_none(2);
            methods::betavariate(_interp, &this, &alpha, &beta)
        }

        fn paretovariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PARETOVARIATE, args)?;
            let this = b.take_or_none(0);
            let alpha = b.take_or_none(1);
            methods::paretovariate(_interp, &this, &alpha)
        }

        fn weibullvariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_WEIBULLVARIATE, args)?;
            let this = b.take_or_none(0);
            let alpha = b.take_or_none(1);
            let beta = b.take_or_none(2);
            methods::weibullvariate(_interp, &this, &alpha, &beta)
        }

        fn binomialvariate(args) -> Result<Value> {
            let mut b = args::bind(&SIG_BINOMIALVARIATE, args)?;
            let this = b.take_or_none(0);
            let n = b.take_or(1, || Value::int(1));
            let p = b.take_or(2, || Value::float(0.5));
            methods::binomialvariate(_interp, &this, &n, &p)
        }
    }

    class SystemRandom("Random") {
        constants {
            "__doc__" => Value::string(docs::SYSTEM_RANDOM_DOC),
        }

        fn random(args) -> Result<Value> {
            args::bind(&SIG_SYS_RANDOM, args)?;
            dispatch::sys_random().map(Value::float)
        }

        fn getrandbits(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SYS_GETRANDBITS, args)?;
            let k = b.take_or_none(1);
            methods::sys_getrandbits(_interp, &k)
        }

        fn randbytes(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SYS_RANDBYTES, args)?;
            let n = b.take_or_none(1);
            methods::sys_randbytes(_interp, &n)
        }

        fn seed(args) -> Result<Value> {
            require_self("SystemRandom.seed", args)?;
            Ok(Value::none())
        }

        fn _notimplemented(args) -> Result<Value> {
            require_self("SystemRandom._notimplemented", args)?;
            Err(PyError::named(
                "NotImplementedError",
                "System entropy source does not have state.",
            ))
        }
    }
}
