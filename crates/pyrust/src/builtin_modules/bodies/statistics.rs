// `statistics` module — 100% native-Rust Python 3.12.3 implementation (issues
// #2811, #3472); there is no Python-source injection.
//
// CPython 3.12's `statistics` is the pure-Python `Lib/statistics.py`.  This
// file ports it function by function:
//
//  * `exact.rs` — `_exact_ratio`, `_sum`, `_ss`, `_coerce`, `_convert` and the
//    correctly rounded square roots.  Sums are accumulated exactly as `BigInt`
//    rationals keyed by denominator, so `mean` / `variance` / `stdev` /
//    `harmonic_mean` preserve `int`, `float`, `Fraction` and `Decimal` results
//    with no catastrophic cancellation, and a NaN / infinity propagates as the
//    value its own type produces.
//  * `functions.rs` — the averages, medians, modes, `quantiles`, the measures
//    of spread, `covariance`, `correlation` (linear and ranked),
//    `linear_regression`.
//  * `normal_dist.rs` — `NormalDist`.
//  * `ops.rs` — interpreter plumbing and the `gil_local!` registry of the
//    module's classes.
//
// Generic arithmetic (`x - c`, `w / x`, `(a + b) / 2`, ...) goes through the
// interpreter's operators exactly like the Python source, so every numeric
// type and every error message matches CPython.  `_statistics._normal_dist_inv_cdf`
// is the native AS 241 implementation shared with the `_statistics` shim.
//
// Namespace differences that are deliberate: CPython's module also binds
// `math`, `numbers`, `random`, `sys`, `Fraction`, `Decimal` and the other
// imported helpers (`count`, `groupby`, `repeat`, ...) plus its private
// helpers (`_sum`, `_ss`, `_coerce`, ...).  Binding the imports would pull the
// pure-Python `decimal`, `fractions` and `random` modules into every
// `import statistics`; they are imported lazily by the code paths that need
// them.  `LinearRegression`, `_SQRT2`, `_sqrt_bit_width` and
// `_normal_dist_inv_cdf` are bound.
//
// Reference: <https://docs.python.org/3.12/library/statistics.html>

use pyrust_derive::pyrust_module;

use crate::builtin_modules::json::args::{self, Sig};
use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::Interpreter;
use crate::interpreter::builtin_args::PyValueRef;
use crate::value::{Value, ValueKind};

#[path = "statistics/docs.rs"]
mod docs;
#[path = "statistics/exact.rs"]
mod exact;
#[path = "statistics/functions.rs"]
mod functions;
#[path = "statistics/normal_dist.rs"]
mod normal_dist;
#[path = "statistics/ops.rs"]
mod ops;

use ops::{ModuleClasses, call_kw, truthy, type_error};

/// `__all__`, in CPython's order.
const ALL: [&str; 20] = [
    "NormalDist",
    "StatisticsError",
    "correlation",
    "covariance",
    "fmean",
    "geometric_mean",
    "harmonic_mean",
    "linear_regression",
    "mean",
    "median",
    "median_grouped",
    "median_high",
    "median_low",
    "mode",
    "multimode",
    "pstdev",
    "pvariance",
    "quantiles",
    "stdev",
    "variance",
];

fn slots_dict() -> Value {
    let mut slots = crate::value::PyDict::default();
    slots.insert(
        crate::value::PyKey::str_from("_mu"),
        Value::string("Arithmetic mean of a normal distribution"),
    );
    slots.insert(
        crate::value::PyKey::str_from("_sigma"),
        Value::string("Standard deviation of a normal distribution"),
    );
    Value::dict(slots)
}

fn all_list() -> Value {
    Value::list(ALL.iter().map(Value::string).collect())
}

// The `def`s of `Lib/statistics.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording.
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

const SIG_DATA: [Sig; 6] = [
    sig("mean", &["data"], 0, &[]),
    sig("geometric_mean", &["data"], 0, &[]),
    sig("median", &["data"], 0, &[]),
    sig("median_low", &["data"], 0, &[]),
    sig("median_high", &["data"], 0, &[]),
    sig("mode", &["data"], 0, &[]),
];
const SIG_MULTIMODE: Sig = sig("multimode", &["data"], 0, &[]);
const SIG_FMEAN: Sig = sig("fmean", &["data", "weights"], 1, &[]);
const SIG_HARMONIC_MEAN: Sig = sig("harmonic_mean", &["data", "weights"], 1, &[]);
const SIG_MEDIAN_GROUPED: Sig = sig("median_grouped", &["data", "interval"], 1, &[]);
const SIG_QUANTILES: Sig = sig("quantiles", &["data"], 0, &["n", "method"]);
const SIG_VARIANCE: Sig = sig("variance", &["data", "xbar"], 1, &[]);
const SIG_PVARIANCE: Sig = sig("pvariance", &["data", "mu"], 1, &[]);
const SIG_STDEV: Sig = sig("stdev", &["data", "xbar"], 1, &[]);
const SIG_PSTDEV: Sig = sig("pstdev", &["data", "mu"], 1, &[]);
const SIG_COVARIANCE: Sig = sig("covariance", &["x", "y"], 0, &[]);
const SIG_CORRELATION: Sig = sig("correlation", &["x", "y"], 0, &["method"]);
const SIG_LINEAR_REGRESSION: Sig = sig("linear_regression", &["x", "y"], 0, &["proportional"]);

const SIG_INIT: Sig = sig("NormalDist.__init__", &["self", "mu", "sigma"], 2, &[]);
const SIG_FROM_SAMPLES: Sig = sig("NormalDist.from_samples", &["cls", "data"], 0, &[]);
const SIG_SAMPLES: Sig = sig("NormalDist.samples", &["self", "n"], 0, &["seed"]);
const SIG_PDF: Sig = sig("NormalDist.pdf", &["self", "x"], 0, &[]);
const SIG_CDF: Sig = sig("NormalDist.cdf", &["self", "x"], 0, &[]);
const SIG_INV_CDF: Sig = sig("NormalDist.inv_cdf", &["self", "p"], 0, &[]);
const SIG_ND_QUANTILES: Sig = sig("NormalDist.quantiles", &["self", "n"], 1, &[]);
const SIG_OVERLAP: Sig = sig("NormalDist.overlap", &["self", "other"], 0, &[]);
const SIG_ZSCORE: Sig = sig("NormalDist.zscore", &["self", "x"], 0, &[]);
const SIG_ADD: Sig = sig("NormalDist.__add__", &["x1", "x2"], 0, &[]);
const SIG_SUB: Sig = sig("NormalDist.__sub__", &["x1", "x2"], 0, &[]);
const SIG_MUL: Sig = sig("NormalDist.__mul__", &["x1", "x2"], 0, &[]);
const SIG_TRUEDIV: Sig = sig("NormalDist.__truediv__", &["x1", "x2"], 0, &[]);
const SIG_POS: Sig = sig("NormalDist.__pos__", &["x1"], 0, &[]);
const SIG_NEG: Sig = sig("NormalDist.__neg__", &["x1"], 0, &[]);
const SIG_RADD: Sig = sig("NormalDist.__radd__", &["x1", "x2"], 0, &[]);
const SIG_RSUB: Sig = sig("NormalDist.__rsub__", &["x1", "x2"], 0, &[]);
const SIG_RMUL: Sig = sig("NormalDist.__rmul__", &["x1", "x2"], 0, &[]);
const SIG_EQ: Sig = sig("NormalDist.__eq__", &["x1", "x2"], 0, &[]);
const SIG_SELF: [Sig; 3] = [
    sig("NormalDist.__hash__", &["self"], 0, &[]),
    sig("NormalDist.__repr__", &["self"], 0, &[]),
    sig("NormalDist.__getstate__", &["self"], 0, &[]),
];
const SIG_SETSTATE: Sig = sig("NormalDist.__setstate__", &["self", "state"], 0, &[]);

/// CPython's `got some positional-only arguments passed as keyword
/// arguments` check for `def f(x, y, /)`.
fn reject_keywords(qualname: &str, args: &[ExpandedCallArg], names: &[&str]) -> Result<()> {
    let passed: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| args.iter().any(|a| a.name.as_deref() == Some(*name)))
        .collect();
    if passed.is_empty() {
        return Ok(());
    }
    Err(type_error(format!(
        "{qualname}() got some positional-only arguments passed as keyword arguments: '{}'",
        passed.join(", ")
    )))
}

/// `bound` value of an optional parameter whose default is `None`.
fn none_default(value: Option<Value>) -> Option<Value> {
    value.filter(|v| !v.is_none())
}

/// Wire what the CPython source gets from its imports and module level:
/// `LinearRegression`, `_SQRT2`, `_sqrt_bit_width`, `_normal_dist_inv_cdf`,
/// and the class registry native code uses to build results.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    let linear_regression = (|| -> Result<Value> {
        let namedtuple = ops::module_attr(interp, "collections", "namedtuple")?;
        call_kw(
            interp,
            namedtuple,
            &[
                Value::string("LinearRegression"),
                Value::tuple(vec![Value::string("slope"), Value::string("intercept")]),
            ],
            &[("module", Value::string("statistics"))],
        )
    })();
    let Ok(linear_regression) = linear_regression else {
        return;
    };
    let error = module_rc.borrow().attrs.get("StatisticsError").cloned();
    let normal_dist = module_rc.borrow().attrs.get("NormalDist").cloned();
    if let (Some(error), Some(normal_dist)) = (error, normal_dist)
        && let (ValueKind::PyClass(error), ValueKind::PyClass(normal_dist)) =
            (error.kind(), normal_dist.kind())
    {
        ops::register_classes(ModuleClasses {
            error: error.clone(),
            normal_dist: normal_dist.clone(),
            linear_regression: linear_regression.clone(),
        });
    }
    module_rc
        .borrow_mut()
        .insert_attr("LinearRegression".to_string(), linear_regression);
    finish_normal_dist(module_rc);
    if let Ok(inv_cdf) = ops::module_attr(interp, "_statistics", "_normal_dist_inv_cdf") {
        module_rc
            .borrow_mut()
            .insert_attr("_normal_dist_inv_cdf".to_string(), inv_cdf);
    }
    reorder(&mut module_rc.borrow_mut().attrs, MODULE_ORDER);
}

/// CPython's `vars(NormalDist)` order (`_mu` / `_sigma` last).
const CLASS_ORDER: &[&str] = &[
    "__module__",
    "__doc__",
    "__slots__",
    "__init__",
    "from_samples",
    "samples",
    "pdf",
    "cdf",
    "inv_cdf",
    "quantiles",
    "overlap",
    "zscore",
    "mean",
    "median",
    "mode",
    "stdev",
    "variance",
    "__add__",
    "__sub__",
    "__mul__",
    "__truediv__",
    "__pos__",
    "__neg__",
    "__radd__",
    "__rsub__",
    "__rmul__",
    "__eq__",
    "__hash__",
    "__repr__",
    "__getstate__",
    "__setstate__",
];

/// CPython's `vars(statistics)` order for the names this module binds.
const MODULE_ORDER: &[&str] = &[
    "_SQRT2",
    "StatisticsError",
    "_sqrt_bit_width",
    "mean",
    "fmean",
    "geometric_mean",
    "harmonic_mean",
    "median",
    "median_low",
    "median_high",
    "median_grouped",
    "mode",
    "multimode",
    "quantiles",
    "variance",
    "pvariance",
    "stdev",
    "pstdev",
    "covariance",
    "correlation",
    "LinearRegression",
    "linear_regression",
    "_normal_dist_inv_cdf",
    "NormalDist",
];

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

/// `NormalDist.__slots__ = {'_mu': ..., '_sigma': ...}`: no instance
/// `__dict__`, and `_mu` / `_sigma` are `member_descriptor`s of the class.
/// Also restores CPython's namespace order.
fn finish_normal_dist(module: &std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>) {
    let class = module.borrow().attrs.get("NormalDist").cloned();
    let Some(ValueKind::PyClass(class)) = class.as_ref().map(Value::kind) else {
        return;
    };
    let descriptors: Vec<(&str, Value)> = ["_mu", "_sigma"]
        .into_iter()
        .map(|slot| {
            (
                slot,
                pyrust_builtins::member_descriptor::member_descriptor(slot, class),
            )
        })
        .collect();
    let mut class = class.borrow_mut();
    class.slots = Some(["_mu", "_sigma"].into_iter().map(str::to_string).collect());
    reorder(&mut class.attrs, CLASS_ORDER);
    for (slot, descriptor) in descriptors {
        class.attrs.insert(slot.to_string(), descriptor);
    }
    class.bump_mutation_version();
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_list(),
        "_SQRT2" => Value::float(normal_dist::SQRT2),
        "_sqrt_bit_width" => Value::int(109),
    }

    exception StatisticsError(ValueError);

    fn mean(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DATA[0], args)?;
        let data = b.take_or_none(0);
        functions::mean(_interp, &data)
    }

    fn fmean(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FMEAN, args)?;
        let data = b.take_or_none(0);
        let weights = none_default(b.take(1));
        functions::fmean(_interp, &data, weights.as_ref())
    }

    fn geometric_mean(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DATA[1], args)?;
        let data = b.take_or_none(0);
        functions::geometric_mean(_interp, &data)
    }

    fn harmonic_mean(args) -> Result<Value> {
        let mut b = args::bind(&SIG_HARMONIC_MEAN, args)?;
        let data = b.take_or_none(0);
        let weights = none_default(b.take(1));
        functions::harmonic_mean(_interp, &data, weights.as_ref())
    }

    fn median(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DATA[2], args)?;
        let data = b.take_or_none(0);
        functions::median(_interp, &data)
    }

    fn median_low(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DATA[3], args)?;
        let data = b.take_or_none(0);
        functions::median_low(_interp, &data)
    }

    fn median_high(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DATA[4], args)?;
        let data = b.take_or_none(0);
        functions::median_high(_interp, &data)
    }

    fn median_grouped(args) -> Result<Value> {
        let mut b = args::bind(&SIG_MEDIAN_GROUPED, args)?;
        let data = b.take_or_none(0);
        let interval = b.take_or(1, || Value::float(1.0));
        functions::median_grouped(_interp, &data, &interval)
    }

    fn mode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DATA[5], args)?;
        let data = b.take_or_none(0);
        functions::mode(_interp, &data)
    }

    fn multimode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_MULTIMODE, args)?;
        let data = b.take_or_none(0);
        functions::multimode(_interp, &data)
    }

    fn quantiles(args) -> Result<Value> {
        let mut b = args::bind(&SIG_QUANTILES, args)?;
        let data = b.take_or_none(0);
        let n = b.take_or(1, || Value::int(4));
        let method = b.take_or(2, || Value::string("exclusive"));
        functions::quantiles(_interp, &data, &n, &method)
    }

    fn variance(args) -> Result<Value> {
        let mut b = args::bind(&SIG_VARIANCE, args)?;
        let data = b.take_or_none(0);
        let xbar = none_default(b.take(1));
        functions::variance(_interp, &data, &xbar)
    }

    fn pvariance(args) -> Result<Value> {
        let mut b = args::bind(&SIG_PVARIANCE, args)?;
        let data = b.take_or_none(0);
        let mu = none_default(b.take(1));
        functions::pvariance(_interp, &data, &mu)
    }

    fn stdev(args) -> Result<Value> {
        let mut b = args::bind(&SIG_STDEV, args)?;
        let data = b.take_or_none(0);
        let xbar = none_default(b.take(1));
        functions::stdev(_interp, &data, &xbar)
    }

    fn pstdev(args) -> Result<Value> {
        let mut b = args::bind(&SIG_PSTDEV, args)?;
        let data = b.take_or_none(0);
        let mu = none_default(b.take(1));
        functions::pstdev(_interp, &data, &mu)
    }

    fn covariance(args) -> Result<Value> {
        reject_keywords("covariance", args, &["x", "y"])?;
        let mut b = args::bind(&SIG_COVARIANCE, args)?;
        let x = b.take_or_none(0);
        let y = b.take_or_none(1);
        functions::covariance(_interp, &x, &y)
    }

    fn correlation(args) -> Result<Value> {
        reject_keywords("correlation", args, &["x", "y"])?;
        let mut b = args::bind(&SIG_CORRELATION, args)?;
        let x = b.take_or_none(0);
        let y = b.take_or_none(1);
        let method = b.take_or(2, || Value::string("linear"));
        functions::correlation(_interp, &x, &y, &method)
    }

    fn linear_regression(args) -> Result<Value> {
        reject_keywords("linear_regression", args, &["x", "y"])?;
        let mut b = args::bind(&SIG_LINEAR_REGRESSION, args)?;
        let x = b.take_or_none(0);
        let y = b.take_or_none(1);
        let proportional = b.take_or(2, || Value::bool_(false));
        let proportional = truthy(_interp, &proportional)?;
        functions::linear_regression(_interp, &x, &y, proportional)
    }

    class NormalDist {
        constants {
            "__doc__" => Value::string(docs::NORMAL_DIST_DOC),
            "__slots__" => slots_dict(),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_INIT, args)?;
            let this = b.take_or_none(0);
            let mu = b.take_or(1, || Value::float(0.0));
            let sigma = b.take_or(2, || Value::float(1.0));
            normal_dist::init(_interp, &this, &mu, &sigma)?;
            Ok(Value::none())
        }

        #[classmethod]
        fn from_samples(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FROM_SAMPLES, args)?;
            let cls = b.take_or_none(0);
            let data = b.take_or_none(1);
            normal_dist::from_samples(_interp, &cls, &data)
        }

        fn samples(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SAMPLES, args)?;
            let this = b.take_or_none(0);
            let n = b.take_or_none(1);
            let seed = b.take_or_none(2);
            normal_dist::samples(_interp, &this, &n, &seed)
        }

        fn pdf(args) -> Result<Value> {
            let mut b = args::bind(&SIG_PDF, args)?;
            let this = b.take_or_none(0);
            let x = b.take_or_none(1);
            normal_dist::pdf(_interp, &this, &x)
        }

        fn cdf(args) -> Result<Value> {
            let mut b = args::bind(&SIG_CDF, args)?;
            let this = b.take_or_none(0);
            let x = b.take_or_none(1);
            normal_dist::cdf(_interp, &this, &x)
        }

        fn inv_cdf(args) -> Result<Value> {
            let mut b = args::bind(&SIG_INV_CDF, args)?;
            let this = b.take_or_none(0);
            let p = b.take_or_none(1);
            normal_dist::inv_cdf(_interp, &this, &p)
        }

        fn quantiles(args) -> Result<Value> {
            let mut b = args::bind(&SIG_ND_QUANTILES, args)?;
            let this = b.take_or_none(0);
            let n = b.take_or(1, || Value::int(4));
            normal_dist::quantiles(_interp, &this, &n)
        }

        fn overlap(args) -> Result<Value> {
            let mut b = args::bind(&SIG_OVERLAP, args)?;
            let this = b.take_or_none(0);
            let other = b.take_or_none(1);
            normal_dist::overlap(_interp, &this, &other)
        }

        fn zscore(args) -> Result<Value> {
            let mut b = args::bind(&SIG_ZSCORE, args)?;
            let this = b.take_or_none(0);
            let x = b.take_or_none(1);
            normal_dist::zscore(_interp, &this, &x)
        }

        #[getter]
        fn mean(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_mu")
        }

        #[getter]
        fn median(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_mu")
        }

        #[getter]
        fn mode(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_mu")
        }

        #[getter]
        fn stdev(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_sigma")
        }

        #[getter]
        fn variance(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            normal_dist::variance(_interp, self_val.0)
        }

        fn __add__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_ADD, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::add(_interp, &x1, &x2)
        }

        fn __sub__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SUB, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::sub(_interp, &x1, &x2)
        }

        fn __mul__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MUL, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::mul(_interp, &x1, &x2)
        }

        fn __truediv__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_TRUEDIV, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::truediv(_interp, &x1, &x2)
        }

        fn __pos__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_POS, args)?;
            let x1 = b.take_or_none(0);
            normal_dist::pos(_interp, &x1)
        }

        fn __neg__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_NEG, args)?;
            let x1 = b.take_or_none(0);
            normal_dist::neg(_interp, &x1)
        }

        fn __radd__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_RADD, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::add(_interp, &x1, &x2)
        }

        fn __rsub__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_RSUB, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::rsub(_interp, &x1, &x2)
        }

        fn __rmul__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_RMUL, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::mul(_interp, &x1, &x2)
        }

        fn __eq__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EQ, args)?;
            let x1 = b.take_or_none(0);
            let x2 = b.take_or_none(1);
            normal_dist::eq(_interp, &x1, &x2)
        }

        fn __hash__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[0], args)?;
            let this = b.take_or_none(0);
            normal_dist::hash(_interp, &this)
        }

        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[1], args)?;
            let this = b.take_or_none(0);
            normal_dist::repr_of(_interp, &this)
        }

        fn __getstate__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[2], args)?;
            let this = b.take_or_none(0);
            normal_dist::getstate(_interp, &this)
        }

        fn __setstate__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SETSTATE, args)?;
            let this = b.take_or_none(0);
            let state = b.take_or_none(1);
            normal_dist::setstate(_interp, &this, &state)
        }
    }
}
