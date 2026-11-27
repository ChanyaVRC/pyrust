// `fractions` module — 100% native-Rust Python 3.12 implementation (issues
// #2810, #3412); there is no Python-source injection.
//
// CPython 3.12's `fractions` is the pure-Python `Lib/fractions.py`: a single
// public class, `Fraction(numbers.Rational)`, an immutable rational number
// whose `_numerator` / `_denominator` slots hold Python ints.  Here each
// instance carries an invisible native payload (`FractionState`) with the two
// `num_bigint::BigInt`s, and every method is a Rust port of its CPython
// counterpart: the same algorithms (Knuth's gcd-reduced arithmetic,
// `limit_denominator`'s continued fractions, the round-half-even helpers,
// `_hash_algorithm`), the same mixed-type dispatch (`_operator_fallbacks`'
// `forward` / `reverse`, `_richcmp`, `__eq__` against `int`, `float`,
// `complex`, `Decimal` and other `numbers.Rational`s) and the same error
// messages.
//
// Arithmetic is exact `BigInt` work with no interpreter round-trip; the
// interpreter is only consulted where CPython dispatches dynamically (a
// `float` / `complex` fallback operator, a user `numbers.Rational`'s
// properties, an overridden `__str__` / `__float__` on a subclass).  Strings
// are parsed by a scanner equivalent to `_RATIONAL_FORMAT` (`parse.rs`) and
// `__format__` re-implements `_FLOAT_FORMAT_SPECIFICATION_MATCHER` and the
// 3.12 float-style formatter (`format.rs`), so importing `fractions` no longer
// pulls in the pure-Python `re` module.
//
// `post_load_init` makes `Fraction` a real subclass of `numbers.Rational`
// with the `abc.ABCMeta` metaclass, restores CPython's class and module
// namespace order, and records the class generation in a `gil_local!` list so
// native code can tell an exact `Fraction` from a subclass.
//
// Namespace differences that are deliberate: CPython's module also binds
// `Decimal`, `functools`, `re`, `_RATIONAL_FORMAT` and
// `_FLOAT_FORMAT_SPECIFICATION_MATCHER`.  Binding them would import the
// pure-Python `decimal`, `functools` and `re` modules (over half a second in a
// debug build) on every `import fractions`, so they are omitted; `decimal` is
// imported lazily by the code paths that need `Decimal`.  The class-body
// helper `Fraction._operator_fallbacks` (a closure factory) is omitted too.
//
// Reference: <https://docs.python.org/3/library/fractions.html>

use std::cell::RefCell;
use std::rc::Rc;

use num_bigint::BigInt;
use num_traits::{One, Signed, Zero};
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, class_is_subclass_of, coerce_subclass_backing, render_value_repr,
    value_type_name_str,
};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyModule, PySet, Value, ValueKind};

#[path = "fractions/args.rs"]
mod args;
#[path = "fractions/arith.rs"]
mod arith;
#[path = "fractions/format.rs"]
mod format;
#[path = "fractions/parse.rs"]
mod parse;

use args::{Sig, bind, type_error};
use arith::{divmod_floor, floor_div, floor_mod, gcd, int_str, pow, pow10, zero_division};

type ClassRc = Rc<RefCell<PyClass>>;

const MODULE_DOC: &str = "Fraction, infinite-precision, rational numbers.";

const FRACTION_DOC: &str = "This class implements rational numbers.

    In the two-argument form of the constructor, Fraction(8, 6) will
    produce a rational number equivalent to 4/3. Both arguments must
    be Rational. The numerator defaults to 0 and the denominator
    defaults to 1 so that Fraction(3) == 3 and Fraction() == 0.

    Fractions can also be constructed from:

      - numeric strings similar to those accepted by the
        float constructor (for example, '-2.3' or '1e10')

      - strings of the form '123/456'

      - float and Decimal instances

      - other Rational instances (including integers)

    ";

/// CPython's `vars(Fraction)` order (minus `_operator_fallbacks`).
const CLASS_ORDER: &[&str] = &[
    "__module__",
    "__doc__",
    "__slots__",
    "__new__",
    "from_float",
    "from_decimal",
    "_from_coprime_ints",
    "is_integer",
    "as_integer_ratio",
    "limit_denominator",
    "numerator",
    "denominator",
    "__repr__",
    "__str__",
    "__format__",
    "_add",
    "__add__",
    "__radd__",
    "_sub",
    "__sub__",
    "__rsub__",
    "_mul",
    "__mul__",
    "__rmul__",
    "_div",
    "__truediv__",
    "__rtruediv__",
    "_floordiv",
    "__floordiv__",
    "__rfloordiv__",
    "_divmod",
    "__divmod__",
    "__rdivmod__",
    "_mod",
    "__mod__",
    "__rmod__",
    "__pow__",
    "__rpow__",
    "__pos__",
    "__neg__",
    "__abs__",
    "__int__",
    "__trunc__",
    "__floor__",
    "__ceil__",
    "__round__",
    "__hash__",
    "__eq__",
    "_richcmp",
    "__lt__",
    "__gt__",
    "__le__",
    "__ge__",
    "__bool__",
    "__reduce__",
    "__copy__",
    "__deepcopy__",
    "_denominator",
    "_numerator",
    "__abstractmethods__",
    "_abc_impl",
];

/// CPython's module namespace order for the names this module binds.
const MODULE_ORDER: &[&str] = &[
    "math",
    "numbers",
    "operator",
    "sys",
    "__all__",
    "_PyHASH_MODULUS",
    "_PyHASH_INF",
    "_hash_algorithm",
    "_round_to_exponent",
    "_round_to_figures",
    "Fraction",
];

// ── Instance state and class generations ─────────────────────────────────────

/// The value of one `Fraction`: CPython's `_numerator` / `_denominator`
/// slots.  Normally in lowest terms with a positive denominator, but
/// `_from_coprime_ints` stores whatever it is given, exactly like CPython.
struct Ratio {
    num: BigInt,
    den: BigInt,
}

impl Ratio {
    fn new(num: BigInt, den: BigInt) -> Rc<Self> {
        Rc::new(Self { num, den })
    }

    fn int(num: BigInt) -> Rc<Self> {
        Self::new(num, BigInt::one())
    }
}

/// Native payload of a `Fraction` instance.  The `Rc` lets callers keep the
/// value past the instance-state borrow.
struct FractionState(Rc<Ratio>);

const TYPE_NAME: &str = "Fraction";

// Per-interpreter state (`gil_local!`):
// (a) Why: native methods have no module reference, yet they must build
//     results of the module's `Fraction` class and tell an exact `Fraction`
//     from a subclass.
// (b) Every generation is kept (a re-import after `del sys.modules[...]`
//     creates a new class); results use the newest one, like CPython's
//     module-global `Fraction` lookup after a reload.
pyrust_core::gil_local! {
    static FRACTION_CLASSES: RefCell<Vec<ClassRc>> = const { RefCell::new(Vec::new()) };
}

fn fraction_class() -> Result<ClassRc> {
    FRACTION_CLASSES
        .with(|classes| classes.borrow().last().cloned())
        .ok_or_else(|| PyError::runtime("fractions is not initialised".to_string()))
}

fn is_fraction_class(class: &ClassRc) -> bool {
    FRACTION_CLASSES.with(|classes| classes.borrow().iter().any(|c| Rc::ptr_eq(c, class)))
}

fn is_fraction_subclass(class: &ClassRc) -> bool {
    FRACTION_CLASSES.with(|classes| {
        classes
            .borrow()
            .iter()
            .any(|c| class_is_subclass_of(class, c))
    })
}

/// `type(value) is Fraction`.
fn is_exact_fraction(value: &Value) -> bool {
    match value.kind() {
        ValueKind::PyInstance(instance) => is_fraction_class(&instance.borrow().class),
        _ => false,
    }
}

/// The value of a `Fraction` (or subclass) instance.
fn ratio_of(value: &Value) -> Option<Rc<Ratio>> {
    if !matches!(value.kind(), ValueKind::PyInstance(_)) {
        return None;
    }
    with_native_state::<FractionState, _>(value, TYPE_NAME, |state| Ok(Rc::clone(&state.0))).ok()
}

/// `value._numerator` (or another slot) on something that may not be a
/// `Fraction`: CPython's `AttributeError` when it is not.
fn slot_ratio(value: &Value, slot: &str) -> Result<Rc<Ratio>> {
    ratio_of(value).ok_or_else(|| {
        PyError::attribute_error(
            format!(
                "'{}' object has no attribute '{slot}'",
                value_type_name_str(value)
            ),
            Some(slot.to_string()),
            Some(value.clone()),
        )
    })
}

/// A new `cls` instance holding `num / den` verbatim.
fn new_instance(cls: &ClassRc, num: BigInt, den: BigInt) -> Result<Value> {
    let instance = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&instance, FractionState(Ratio::new(num, den)))?;
    Ok(instance)
}

/// `Fraction._from_coprime_ints(num, den)` on the module's `Fraction`.
fn new_fraction(num: BigInt, den: BigInt) -> Result<Value> {
    new_instance(&fraction_class()?, num, den)
}

/// `super(Fraction, cls).__new__(cls)`'s check that `cls` is a `Fraction`
/// subtype.
fn fraction_subtype(cls: &Value) -> Result<ClassRc> {
    match cls.kind() {
        ValueKind::PyClass(class) if is_fraction_subclass(class) => Ok(Rc::clone(class)),
        _ => Err(type_error(
            "super(type, obj): obj must be an instance or subtype of type",
        )),
    }
}

// ── Value helpers ─────────────────────────────────────────────────────────────

fn int_value(n: BigInt) -> Value {
    pyrust_core::value_from_bigint(n)
}

fn int_ref(n: &BigInt) -> Value {
    int_value(n.clone())
}

/// `type(value) is int`.
fn exact_int(value: &Value) -> Option<BigInt> {
    match value.kind() {
        ValueKind::Int(n) => Some(BigInt::from(n)),
        ValueKind::BigInt(b) => Some(b.clone()),
        _ => None,
    }
}

/// An exact `int` or `bool`.
fn int_or_bool(value: &Value) -> Option<BigInt> {
    match value.kind() {
        ValueKind::Bool(b) => Some(BigInt::from(u8::from(b))),
        _ => exact_int(value),
    }
}

/// `isinstance(value, int)`.
fn is_int(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => true,
        ValueKind::PyInstance(_) => coerce_subclass_backing(value, &[]).is_some_and(|b| {
            matches!(
                b.kind(),
                ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
            )
        }),
        _ => false,
    }
}

/// The value of `value` when `isinstance(value, float)`.
fn float_of(value: &Value) -> Option<f64> {
    match value.kind() {
        ValueKind::Float(f) => Some(f),
        ValueKind::PyInstance(_) => match coerce_subclass_backing(value, &[])?.kind() {
            ValueKind::Float(f) => Some(f),
            _ => None,
        },
        _ => None,
    }
}

/// `isinstance(value, complex)`.
fn is_complex(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Complex(..) => true,
        ValueKind::PyInstance(_) => coerce_subclass_backing(value, &[])
            .is_some_and(|b| matches!(b.kind(), ValueKind::Complex(..))),
        _ => false,
    }
}

/// The text of `value` when `isinstance(value, str)`.
fn str_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if !matches!(value.kind(), ValueKind::PyInstance(_)) {
        return None;
    }
    coerce_subclass_backing(value, &[])?
        .as_str()
        .map(str::to_string)
}

/// Whether an ABC check has to ask the interpreter: only instances and
/// classes can be virtual subclasses of the `numbers` ABCs beyond the
/// built-in registrations handled inline.
fn needs_abc_check(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::PyInstance(_) | ValueKind::PyClass(_)
    )
}

fn call(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_builtin(interp: &mut Interpreter, name: &'static str, args: &[Value]) -> Result<Value> {
    call(interp, Value::builtin_function(name), args)
}

fn py_isinstance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let result = call_builtin(interp, "isinstance", &[obj.clone(), cls.clone()])?;
    interp.truthy_value(&result)
}

/// `isinstance(value, numbers.<name>)` through the ABC machinery.
fn isinstance_numbers(interp: &mut Interpreter, value: &Value, name: &str) -> Result<bool> {
    let numbers = interp.load_module("numbers")?;
    let abc = interp.get_attr(&numbers, name)?;
    py_isinstance(interp, value, &abc)
}

/// `isinstance(value, numbers.Integral)`.
fn is_integral(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    match value.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => Ok(true),
        _ if needs_abc_check(value) => isinstance_numbers(interp, value, "Integral"),
        _ => Ok(false),
    }
}

/// `isinstance(value, numbers.Real)`.
fn is_real(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    match value.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) | ValueKind::Float(_) => {
            Ok(true)
        }
        _ if needs_abc_check(value) => isinstance_numbers(interp, value, "Real"),
        _ => Ok(false),
    }
}

/// `isinstance(value, numbers.Complex)`.
fn is_complex_number(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    match value.kind() {
        ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Bool(_)
        | ValueKind::Float(_)
        | ValueKind::Complex(..) => Ok(true),
        _ if needs_abc_check(value) => isinstance_numbers(interp, value, "Complex"),
        _ => Ok(false),
    }
}

/// An integer-valued object as a `BigInt` (`operator.index`).
fn index_int(interp: &mut Interpreter, value: &Value) -> Result<BigInt> {
    if let Some(n) = int_or_bool(value) {
        return Ok(n);
    }
    let resolved = interp.value_to_index(value, |v| {
        type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            value_type_name_str(v)
        ))
    })?;
    int_or_bool(&resolved).ok_or_else(|| type_error("__index__ returned non-int"))
}

/// `(value.numerator, value.denominator)` when `isinstance(value,
/// numbers.Rational)`, else `None`.
fn rational_parts(interp: &mut Interpreter, value: &Value) -> Result<Option<Rc<Ratio>>> {
    if let Some(n) = int_or_bool(value) {
        return Ok(Some(Ratio::int(n)));
    }
    if !needs_abc_check(value) {
        return Ok(None);
    }
    if is_exact_fraction(value)
        && let Some(ratio) = ratio_of(value)
    {
        return Ok(Some(ratio));
    }
    if !isinstance_numbers(interp, value, "Rational")? {
        return Ok(None);
    }
    let numerator = interp.get_attr(value, "numerator")?;
    let denominator = interp.get_attr(value, "denominator")?;
    Ok(Some(Ratio::new(
        index_int(interp, &numerator)?,
        index_int(interp, &denominator)?,
    )))
}

/// `float(fraction)`: `Rational.__float__` (`numerator / denominator` as an
/// exact `int` true division) for an exact `Fraction`, the full `float()`
/// protocol for anything else.
fn fraction_float(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if is_exact_fraction(value)
        && let Some(ratio) = ratio_of(value)
    {
        return interp.eval_binary(int_ref(&ratio.num), BinaryOp::Div, int_ref(&ratio.den));
    }
    call_builtin(interp, "float", std::slice::from_ref(value))
}

/// `float(value)` for a `numbers.Rational` operand.
fn rational_float(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if is_exact_fraction(value) {
        return fraction_float(interp, value);
    }
    call_builtin(interp, "float", std::slice::from_ref(value))
}

/// `complex(fraction)` (`Real.__complex__` is `complex(float(self))`).
fn fraction_complex(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    if is_exact_fraction(value) {
        let as_float = fraction_float(interp, value)?;
        return call_builtin(interp, "complex", &[as_float]);
    }
    call_builtin(interp, "complex", std::slice::from_ref(value))
}

/// The exact ratio of a finite `f64` (`float.as_integer_ratio`).
fn float_ratio(f: f64) -> (BigInt, BigInt) {
    if f == 0.0 {
        return (BigInt::zero(), BigInt::one());
    }
    let bits = f.to_bits();
    let negative = bits >> 63 != 0;
    let exp_raw = ((bits >> 52) & 0x7ff) as i64;
    let frac = bits & 0x000f_ffff_ffff_ffff;
    let (mantissa, exp) = if exp_raw == 0 {
        (frac, -1074)
    } else {
        (frac | 0x0010_0000_0000_0000, exp_raw - 1075)
    };
    let trailing = mantissa.trailing_zeros();
    let mantissa = mantissa >> trailing;
    let exp = exp + i64::from(trailing);
    let mut num = BigInt::from(mantissa);
    if negative {
        num = -num;
    }
    if exp >= 0 {
        (num << exp as usize, BigInt::one())
    } else {
        (num, BigInt::one() << (-exp) as usize)
    }
}

/// `float.as_integer_ratio()` including its non-finite errors.
fn float_ratio_checked(f: f64) -> Result<(BigInt, BigInt)> {
    if f.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "cannot convert NaN to integer ratio",
        ));
    }
    if f.is_infinite() {
        return Err(PyError::named(
            "OverflowError",
            "cannot convert Infinity to integer ratio",
        ));
    }
    Ok(float_ratio(f))
}

/// `a, b = pair` for an `as_integer_ratio()` result.
fn unpack_pair(interp: &mut Interpreter, pair: &Value) -> Result<(BigInt, BigInt)> {
    let items: Vec<Value> = match pair.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        ValueKind::List(items) => items.clone(),
        _ => {
            return Err(type_error(format!(
                "cannot unpack non-iterable {} object",
                value_type_name_str(pair)
            )));
        }
    };
    match items.len() {
        2 => Ok((index_int(interp, &items[0])?, index_int(interp, &items[1])?)),
        n if n > 2 => Err(PyError::named(
            "ValueError",
            "too many values to unpack (expected 2)",
        )),
        n => Err(PyError::named(
            "ValueError",
            format!("not enough values to unpack (expected 2, got {n})"),
        )),
    }
}

/// `value.as_integer_ratio()` through the method (float subclasses,
/// `Decimal`).
fn call_as_integer_ratio(interp: &mut Interpreter, value: &Value) -> Result<(BigInt, BigInt)> {
    if let ValueKind::Float(f) = value.kind() {
        return float_ratio_checked(f);
    }
    let method = interp.get_attr(value, "as_integer_ratio")?;
    let pair = call(interp, method, &[])?;
    unpack_pair(interp, &pair)
}

/// `decimal.Decimal`, importing `decimal` on first use.
fn decimal_class(interp: &mut Interpreter) -> Result<Value> {
    let decimal = interp.load_module("decimal")?;
    interp.get_attr(&decimal, "Decimal")
}

/// `isinstance(value, Decimal)`.  Only instances can be `Decimal`s, so
/// `decimal` is not imported for anything else.
fn is_decimal(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if !matches!(value.kind(), ValueKind::PyInstance(_)) {
        return Ok(false);
    }
    let decimal = decimal_class(interp)?;
    py_isinstance(interp, value, &decimal)
}

fn class_name(cls: &Value) -> String {
    match cls.kind() {
        ValueKind::PyClass(class) => class.borrow().name.clone(),
        _ => value_type_name_str(cls).into_owned(),
    }
}

/// `type(value).__name__`.
fn type_name(value: &Value) -> String {
    match value.kind() {
        ValueKind::PyInstance(instance) => instance.borrow().class.borrow().name.clone(),
        _ => value_type_name_str(value).into_owned(),
    }
}

// ── Construction ─────────────────────────────────────────────────────────────

/// The normalising tail of `Fraction.__new__`: reject a zero denominator and
/// divide out the gcd, giving the denominator the sign of a positive number.
fn normalize(num: BigInt, den: BigInt) -> Result<(BigInt, BigInt)> {
    if den.is_zero() {
        return Err(zero_division(&format!("Fraction({}, 0)", int_str(&num)?)));
    }
    let mut g = gcd(&num, &den);
    if den.is_negative() {
        g = -g;
    }
    if g.is_one() {
        return Ok((num, den));
    }
    Ok((num / &g, den / &g))
}

/// `Fraction(<str>)`.
fn parse_str(interp: &mut Interpreter, original: &Value, text: &str) -> Result<(BigInt, BigInt)> {
    let Some(literal) = parse::match_rational(text) else {
        return Err(PyError::named(
            "ValueError",
            format!(
                "Invalid literal for Fraction: {}",
                render_value_repr(interp, original)?
            ),
        ));
    };
    let mut numerator = if literal.num.is_empty() {
        BigInt::zero()
    } else {
        arith::parse_int(&literal.num)?
    };
    let mut denominator = BigInt::one();
    if let Some(denom) = &literal.denom {
        denominator = arith::parse_int(denom)?;
    } else {
        if let Some(decimal) = literal.decimal.as_deref().filter(|d| !d.is_empty()) {
            let decimal: String = decimal.chars().filter(|&c| c != '_').collect();
            let scale = pow10(&BigInt::from(decimal.len()))?;
            numerator = numerator * &scale + arith::parse_int(&decimal)?;
            denominator *= scale;
        }
        if let Some(exp) = &literal.exp {
            let exp = arith::parse_int(exp)?;
            if exp.is_negative() {
                denominator *= pow10(&-exp)?;
            } else {
                numerator *= pow10(&exp)?;
            }
        }
    }
    if literal.negative {
        numerator = -numerator;
    }
    normalize(numerator, denominator)
}

/// `Fraction(numerator)` with the default denominator.
fn single_argument(interp: &mut Interpreter, numerator: &Value) -> Result<(BigInt, BigInt)> {
    if let Some(n) = exact_int(numerator) {
        return Ok((n, BigInt::one()));
    }
    match numerator.kind() {
        ValueKind::Float(f) => return float_ratio_checked(f),
        ValueKind::Str(text) => {
            let text = text.to_string();
            return parse_str(interp, numerator, &text);
        }
        _ => {}
    }
    if let Some(ratio) = rational_parts(interp, numerator)? {
        return Ok((ratio.num.clone(), ratio.den.clone()));
    }
    if float_of(numerator).is_some() || is_decimal(interp, numerator)? {
        // Exact conversion.
        return call_as_integer_ratio(interp, numerator);
    }
    if let Some(text) = str_text(numerator) {
        return parse_str(interp, numerator, &text);
    }
    Err(type_error(
        "argument should be a string or a Rational instance",
    ))
}

/// The body of `Fraction.__new__(cls, numerator=0, denominator=None)`.
fn construct(
    interp: &mut Interpreter,
    cls: &ClassRc,
    numerator: Option<&Value>,
    denominator: Option<&Value>,
) -> Result<Value> {
    let zero = Value::int(0);
    let numerator = numerator.unwrap_or(&zero);
    let Some(denominator) = denominator.filter(|d| !d.is_none()) else {
        let (num, den) = single_argument(interp, numerator)?;
        return new_instance(cls, num, den);
    };
    let (num, den) = match (exact_int(numerator), exact_int(denominator)) {
        // The *very* normal case.
        (Some(num), Some(den)) => (num, den),
        _ => {
            let parts = match rational_parts(interp, numerator)? {
                Some(n) => rational_parts(interp, denominator)?.map(|d| (n, d)),
                None => None,
            };
            let Some((n, d)) = parts else {
                return Err(type_error("both arguments should be Rational instances"));
            };
            (&n.num * &d.den, &d.num * &n.den)
        }
    };
    let (num, den) = normalize(num, den)?;
    new_instance(cls, num, den)
}

/// `cls._from_coprime_ints(num, den)`, dispatching through the class when it
/// is not `Fraction` itself.
fn from_coprime(interp: &mut Interpreter, cls: &Value, num: BigInt, den: BigInt) -> Result<Value> {
    if let ValueKind::PyClass(class) = cls.kind()
        && is_fraction_class(class)
    {
        return new_instance(class, num, den);
    }
    let method = interp.get_attr(cls, "_from_coprime_ints")?;
    call(interp, method, &[int_value(num), int_value(den)])
}

/// `cls.from_float(f)`.
fn from_float(interp: &mut Interpreter, cls: &Value, f: &Value) -> Result<Value> {
    if is_integral(interp, f)? {
        return call(interp, cls.clone(), std::slice::from_ref(f));
    }
    if float_of(f).is_none() {
        return Err(type_error(format!(
            "{}.from_float() only takes floats, not {} ({})",
            class_name(cls),
            render_value_repr(interp, f)?,
            type_name(f)
        )));
    }
    let (num, den) = call_as_integer_ratio(interp, f)?;
    from_coprime(interp, cls, num, den)
}

/// `cls.from_decimal(dec)`.
fn from_decimal(interp: &mut Interpreter, cls: &Value, dec: &Value) -> Result<Value> {
    let decimal = decimal_class(interp)?;
    let dec = if is_integral(interp, dec)? {
        let as_int = call_builtin(interp, "int", std::slice::from_ref(dec))?;
        call(interp, decimal, &[as_int])?
    } else if !py_isinstance(interp, dec, &decimal)? {
        return Err(type_error(format!(
            "{}.from_decimal() only takes Decimals, not {} ({})",
            class_name(cls),
            render_value_repr(interp, dec)?,
            type_name(dec)
        )));
    } else {
        dec.clone()
    };
    let (num, den) = call_as_integer_ratio(interp, &dec)?;
    from_coprime(interp, cls, num, den)
}

// ── Arithmetic ───────────────────────────────────────────────────────────────

/// The operators `_operator_fallbacks` generates `forward` / `reverse` for.
#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    DivMod,
    Mod,
}

/// An unwrapped `Fraction._from_coprime_ints` result.
type Parts = (BigInt, BigInt);

/// `_add` / `_sub`: Knuth, TAOCP, Volume 2, 4.5.1.
fn add_sub(a: &Ratio, b: &Ratio, subtract: bool) -> Result<Parts> {
    let combine = |x: BigInt, y: BigInt| if subtract { x - y } else { x + y };
    let (na, da) = (&a.num, &a.den);
    let (nb, db) = (&b.num, &b.den);
    let g = gcd(da, db);
    if g.is_one() {
        return Ok((combine(na * db, da * nb), da * db));
    }
    let s = floor_div(da, &g)?;
    let t = combine(na * floor_div(db, &g)?, nb * &s);
    let g2 = gcd(&t, &g);
    if g2.is_one() {
        return Ok((t, s * db));
    }
    let num = floor_div(&t, &g2)?;
    Ok((num, s * floor_div(db, &g2)?))
}

/// `_mul`.
fn mul(a: &Ratio, b: &Ratio) -> Result<Parts> {
    let (mut na, mut da) = (a.num.clone(), a.den.clone());
    let (mut nb, mut db) = (b.num.clone(), b.den.clone());
    let g1 = gcd(&na, &db);
    if g1 > BigInt::one() {
        na = floor_div(&na, &g1)?;
        db = floor_div(&db, &g1)?;
    }
    let g2 = gcd(&nb, &da);
    if g2 > BigInt::one() {
        nb = floor_div(&nb, &g2)?;
        da = floor_div(&da, &g2)?;
    }
    Ok((na * nb, db * da))
}

/// `_div`: `_mul` with `b` inverted.
fn div(a: &Ratio, b: &Ratio) -> Result<Parts> {
    let (mut nb, mut db) = (b.num.clone(), b.den.clone());
    if nb.is_zero() {
        return Err(zero_division(&format!("Fraction({}, 0)", int_str(&db)?)));
    }
    let (mut na, mut da) = (a.num.clone(), a.den.clone());
    let g1 = gcd(&na, &nb);
    if g1 > BigInt::one() {
        na = floor_div(&na, &g1)?;
        nb = floor_div(&nb, &g1)?;
    }
    let g2 = gcd(&db, &da);
    if g2 > BigInt::one() {
        da = floor_div(&da, &g2)?;
        db = floor_div(&db, &g2)?;
    }
    let (mut n, mut d) = (na * db, nb * da);
    if d.is_negative() {
        n = -n;
        d = -d;
    }
    Ok((n, d))
}

fn wrap((num, den): Parts) -> Result<Value> {
    new_fraction(num, den)
}

/// A `Fraction(n, d)` built by the normalising constructor.
fn normalized_fraction(num: BigInt, den: BigInt) -> Result<Value> {
    wrap(normalize(num, den)?)
}

/// The purely rational `monomorphic_operator`s.
fn monomorphic(op: ArithOp, a: &Ratio, b: &Ratio) -> Result<Value> {
    match op {
        ArithOp::Add => wrap(add_sub(a, b, false)?),
        ArithOp::Sub => wrap(add_sub(a, b, true)?),
        ArithOp::Mul => wrap(mul(a, b)?),
        ArithOp::Div => wrap(div(a, b)?),
        ArithOp::FloorDiv => Ok(int_value(floor_div(
            &(&a.num * &b.den),
            &(&a.den * &b.num),
        )?)),
        ArithOp::DivMod => {
            let (quotient, n_mod) = divmod_floor(&(&a.num * &b.den), &(&a.den * &b.num))?;
            let remainder = normalized_fraction(n_mod, &a.den * &b.den)?;
            Ok(Value::tuple(vec![int_value(quotient), remainder]))
        }
        ArithOp::Mod => {
            let n_mod = floor_mod(&(&a.num * &b.den), &(&b.num * &a.den))?;
            normalized_fraction(n_mod, &a.den * &b.den)
        }
    }
}

/// The `fallback_operator` (`operator.add`, ..., `divmod`).
fn fallback(interp: &mut Interpreter, op: ArithOp, x: Value, y: Value) -> Result<Value> {
    let binary = match op {
        ArithOp::Add => BinaryOp::Add,
        ArithOp::Sub => BinaryOp::Sub,
        ArithOp::Mul => BinaryOp::Mul,
        ArithOp::Div => BinaryOp::Div,
        ArithOp::FloorDiv => BinaryOp::FloorDiv,
        ArithOp::Mod => BinaryOp::Mod,
        ArithOp::DivMod => return call_builtin(interp, "divmod", &[x, y]),
    };
    interp.eval_binary(x, binary, y)
}

/// `_operator_fallbacks.<locals>.forward(a, b)`.
fn forward(interp: &mut Interpreter, op: ArithOp, a: &Value, b: &Value) -> Result<Value> {
    if let Some(rb) = ratio_of(b) {
        let ra = slot_ratio(a, "_numerator")?;
        return monomorphic(op, &ra, &rb);
    }
    if is_int(b) {
        // `Fraction(b)`.
        let rb = match rational_parts(interp, b)? {
            Some(rb) => rb,
            None => {
                return Err(type_error(
                    "argument should be a string or a Rational instance",
                ));
            }
        };
        let ra = slot_ratio(a, "_numerator")?;
        return monomorphic(op, &ra, &rb);
    }
    if float_of(b).is_some() {
        let fa = fraction_float(interp, a)?;
        return fallback(interp, op, fa, b.clone());
    }
    if is_complex(b) {
        let ca = fraction_complex(interp, a)?;
        return fallback(interp, op, ca, b.clone());
    }
    Ok(Value::not_implemented())
}

/// `_operator_fallbacks.<locals>.reverse(b, a)` for `a <op> b`.
fn reverse(interp: &mut Interpreter, op: ArithOp, b: &Value, a: &Value) -> Result<Value> {
    if let Some(ra) = rational_parts(interp, a)? {
        // Includes ints.
        let rb = slot_ratio(b, "_numerator")?;
        return monomorphic(op, &ra, &rb);
    }
    if is_real(interp, a)? {
        let fa = call_builtin(interp, "float", std::slice::from_ref(a))?;
        let fb = fraction_float(interp, b)?;
        return fallback(interp, op, fa, fb);
    }
    if is_complex_number(interp, a)? {
        let ca = call_builtin(interp, "complex", std::slice::from_ref(a))?;
        let cb = fraction_complex(interp, b)?;
        return fallback(interp, op, ca, cb);
    }
    Ok(Value::not_implemented())
}

/// `a ** b`.
fn power(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    let Some(rb) = rational_parts(interp, b)? else {
        let fa = fraction_float(interp, a)?;
        return interp.eval_binary(fa, BinaryOp::Pow, b.clone());
    };
    if !rb.den.is_one() {
        // A fractional power will generally produce an irrational number.
        let fa = fraction_float(interp, a)?;
        let fb = rational_float(interp, b)?;
        return interp.eval_binary(fa, BinaryOp::Pow, fb);
    }
    let ra = slot_ratio(a, "_numerator")?;
    let exponent = &rb.num;
    if !exponent.is_negative() {
        return new_fraction(pow(&ra.num, exponent)?, pow(&ra.den, exponent)?);
    }
    let exponent = -exponent;
    if ra.num.is_positive() {
        new_fraction(pow(&ra.den, &exponent)?, pow(&ra.num, &exponent)?)
    } else if ra.num.is_zero() {
        Err(zero_division(&format!(
            "Fraction({}, 0)",
            int_str(&pow(&ra.den, &exponent)?)?
        )))
    } else {
        new_fraction(pow(&-&ra.den, &exponent)?, pow(&-&ra.num, &exponent)?)
    }
}

/// `a ** b` reflected (`b` is the `Fraction`).
fn reflected_power(interp: &mut Interpreter, b: &Value, a: &Value) -> Result<Value> {
    let rb = slot_ratio(b, "_denominator")?;
    if rb.den.is_one() && !rb.num.is_negative() {
        // If a is an int, keep it that way if possible.
        return interp.eval_binary(a.clone(), BinaryOp::Pow, int_ref(&rb.num));
    }
    if let Some(ra) = rational_parts(interp, a)? {
        let base = normalized_fraction(ra.num.clone(), ra.den.clone())?;
        return interp.eval_binary(base, BinaryOp::Pow, b.clone());
    }
    if rb.den.is_one() {
        return interp.eval_binary(a.clone(), BinaryOp::Pow, int_ref(&rb.num));
    }
    let fb = fraction_float(interp, b)?;
    interp.eval_binary(a.clone(), BinaryOp::Pow, fb)
}

/// `int(a)` / `math.trunc(a)`: truncation towards zero.
fn truncate(ratio: &Ratio) -> Result<BigInt> {
    if ratio.num.is_negative() {
        Ok(-floor_div(&-&ratio.num, &ratio.den)?)
    } else {
        floor_div(&ratio.num, &ratio.den)
    }
}

/// `round(self)`: round half to even.
fn round_half_even(ratio: &Ratio) -> Result<BigInt> {
    let d = &ratio.den;
    let (floor, remainder) = divmod_floor(&ratio.num, d)?;
    let twice = remainder * 2;
    if &twice < d {
        Ok(floor)
    } else if &twice > d {
        Ok(floor + 1)
    } else if floor_mod(&floor, &BigInt::from(2))?.is_zero() {
        Ok(floor)
    } else {
        Ok(floor + 1)
    }
}

/// `round(self, ndigits)`.
fn round_digits(interp: &mut Interpreter, this: &Value, ndigits: &Value) -> Result<Value> {
    if is_exact_fraction(this)
        && let Some(nd) = int_or_bool(ndigits)
        && let Some(ratio) = ratio_of(this)
    {
        let shift = pow10(&nd.abs())?;
        let shifted = Ratio::int(shift.clone());
        return if nd.is_positive() {
            let (num, den) = mul(&ratio, &shifted)?;
            let rounded = round_half_even(&Ratio { num, den })?;
            normalized_fraction(rounded, shift)
        } else {
            let (num, den) = div(&ratio, &shifted)?;
            let rounded = round_half_even(&Ratio { num, den })?;
            new_fraction(rounded * shift, BigInt::one())
        };
    }
    // The generic path: CPython's arithmetic on whatever `ndigits` is.
    let magnitude = call_builtin(interp, "abs", std::slice::from_ref(ndigits))?;
    let shift = interp.eval_binary(Value::int(10), BinaryOp::Pow, magnitude)?;
    let positive = interp.eval_binary(ndigits.clone(), BinaryOp::Gt, Value::int(0))?;
    let cls = fraction_class()?;
    if interp.truthy_value(&positive)? {
        let scaled = interp.eval_binary(this.clone(), BinaryOp::Mul, shift.clone())?;
        let rounded = call_builtin(interp, "round", &[scaled])?;
        construct(interp, &cls, Some(&rounded), Some(&shift))
    } else {
        let scaled = interp.eval_binary(this.clone(), BinaryOp::Div, shift.clone())?;
        let rounded = call_builtin(interp, "round", &[scaled])?;
        let product = interp.eval_binary(rounded, BinaryOp::Mul, shift)?;
        construct(interp, &cls, Some(&product), None)
    }
}

/// `limit_denominator(self, max_denominator)`.
fn limit_denominator(interp: &mut Interpreter, this: &Value, max_d: &Value) -> Result<Value> {
    let too_small = match int_or_bool(max_d) {
        Some(m) => m < BigInt::one(),
        None => {
            let lt = interp.eval_binary(max_d.clone(), BinaryOp::Lt, Value::int(1))?;
            interp.truthy_value(&lt)?
        }
    };
    if too_small {
        return Err(PyError::named(
            "ValueError",
            "max_denominator should be at least 1",
        ));
    }
    let ratio = slot_ratio(this, "_denominator")?;
    let fits = match int_or_bool(max_d) {
        Some(m) => ratio.den <= m,
        None => {
            let le = interp.eval_binary(int_ref(&ratio.den), BinaryOp::Le, max_d.clone())?;
            interp.truthy_value(&le)?
        }
    };
    if fits {
        return construct(interp, &fraction_class()?, Some(this), None);
    }
    let max_d = index_int(interp, max_d)?;
    let (mut p0, mut q0, mut p1, mut q1) =
        (BigInt::zero(), BigInt::one(), BigInt::one(), BigInt::zero());
    let (mut n, mut d) = (ratio.num.clone(), ratio.den.clone());
    loop {
        let a = floor_div(&n, &d)?;
        let q2 = &q0 + &a * &q1;
        if q2 > max_d {
            break;
        }
        let p2 = &p0 + &a * &p1;
        p0 = std::mem::replace(&mut p1, p2);
        q0 = std::mem::replace(&mut q1, q2);
        let next_d = &n - &a * &d;
        n = std::mem::replace(&mut d, next_d);
    }
    let k = floor_div(&(&max_d - &q0), &q1)?;
    // Determine which of the candidates (p0+k*p1)/(q0+k*q1) and p1/q1 is
    // closer to self.
    let bound = &q0 + &k * &q1;
    if BigInt::from(2) * &d * &bound <= ratio.den {
        new_fraction(p1, q1)
    } else {
        new_fraction(p0 + k * p1, bound)
    }
}

// ── Comparisons ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum CmpOp {
    Lt,
    Gt,
    Le,
    Ge,
}

impl CmpOp {
    fn holds<T: PartialOrd>(self, x: &T, y: &T) -> bool {
        match self {
            CmpOp::Lt => x < y,
            CmpOp::Gt => x > y,
            CmpOp::Le => x <= y,
            CmpOp::Ge => x >= y,
        }
    }
}

/// `a == b`.
fn equals(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<Value> {
    if let Some(n) = exact_int(b) {
        let ra = slot_ratio(a, "_numerator")?;
        return Ok(Value::bool_(ra.num == n && ra.den.is_one()));
    }
    if let Some(rb) = rational_parts(interp, b)? {
        let ra = slot_ratio(a, "_numerator")?;
        return Ok(Value::bool_(ra.num == rb.num && ra.den == rb.den));
    }
    // isinstance(b, numbers.Complex) and b.imag == 0: b = b.real
    let complex_real = match b.kind() {
        ValueKind::Complex(re, im) => Some((re, im)),
        _ => None,
    };
    let mut b = b.clone();
    if let Some((re, im)) = complex_real {
        if im == 0.0 {
            b = Value::float(re);
        }
    } else if needs_abc_check(&b) && float_of(&b).is_none() && is_complex_number(interp, &b)? {
        let imag = interp.get_attr(&b, "imag")?;
        let zero_imag = interp.eval_binary(imag, BinaryOp::Eq, Value::int(0))?;
        if interp.truthy_value(&zero_imag)? {
            b = interp.get_attr(&b, "real")?;
        }
    }
    if let Some(f) = float_of(&b) {
        if !f.is_finite() {
            // Comparisons with an infinity or nan should behave in the same
            // way for any finite a, so treat a as zero.
            return Ok(Value::bool_(false));
        }
        let ra = slot_ratio(a, "_numerator")?;
        let (n, d) = float_ratio(f);
        return Ok(Value::bool_(ra.num == n && ra.den == d));
    }
    // Since a doesn't know how to compare with b, let's give b a chance to
    // compare itself with a.
    Ok(Value::not_implemented())
}

/// `_richcmp(self, other, op)` for one of the four ordering operators.
fn richcmp(interp: &mut Interpreter, this: &Value, other: &Value, op: CmpOp) -> Result<Value> {
    if let Some(ro) = rational_parts(interp, other)? {
        let rs = slot_ratio(this, "_numerator")?;
        return Ok(Value::bool_(
            op.holds(&(&rs.num * &ro.den), &(&rs.den * &ro.num)),
        ));
    }
    if let Some(f) = float_of(other) {
        if !f.is_finite() {
            return Ok(Value::bool_(op.holds(&0.0, &f)));
        }
        let rs = slot_ratio(this, "_numerator")?;
        let (n, d) = float_ratio(f);
        return Ok(Value::bool_(op.holds(&(&rs.num * &d), &(&rs.den * &n))));
    }
    Ok(Value::not_implemented())
}

/// The exposed `Fraction._richcmp(self, other, op)` with a Python `op`.
fn richcmp_with(
    interp: &mut Interpreter,
    this: &Value,
    other: &Value,
    op: &Value,
) -> Result<Value> {
    if let Some(ro) = rational_parts(interp, other)? {
        let rs = slot_ratio(this, "_numerator")?;
        let x = int_value(&rs.num * &ro.den);
        let y = int_value(&rs.den * &ro.num);
        return call(interp, op.clone(), &[x, y]);
    }
    if let Some(f) = float_of(other) {
        if !f.is_finite() {
            return call(interp, op.clone(), &[Value::float(0.0), other.clone()]);
        }
        let cls = interp.get_attr(this, "__class__")?;
        let converted = from_float(interp, &cls, other)?;
        return call(interp, op.clone(), &[this.clone(), converted]);
    }
    Ok(Value::not_implemented())
}

// ── Presentation ─────────────────────────────────────────────────────────────

/// `str(fraction)` for the state of a `Fraction`.
fn ratio_str(ratio: &Ratio) -> Result<String> {
    if ratio.den.is_one() {
        int_str(&ratio.num)
    } else {
        Ok(format!("{}/{}", int_str(&ratio.num)?, int_str(&ratio.den)?))
    }
}

/// `Fraction.__format__(self, format_spec, /)`.
fn format_value(interp: &mut Interpreter, this: &Value, spec_value: &Value) -> Result<Value> {
    let Some(spec_text) = str_text(spec_value) else {
        if !interp.truthy_value(spec_value)? {
            return Ok(Value::string(interp.render_value_as_str(this)?));
        }
        return Err(type_error(format!(
            "expected string or bytes-like object, got '{}'",
            type_name(spec_value)
        )));
    };
    // Backwards compatibility with existing formatting.
    if spec_text.is_empty() {
        return Ok(Value::string(interp.render_value_as_str(this)?));
    }
    let invalid = |interp: &mut Interpreter, extra: &str| -> Result<PyError> {
        let spec_repr = render_value_repr(interp, spec_value)?;
        let type_repr = render_value_repr(interp, &Value::string(type_name(this)))?;
        Ok(PyError::named(
            "ValueError",
            format!("Invalid format specifier {spec_repr} for object of type {type_repr}{extra}"),
        ))
    };
    let Some(spec) = format::parse_spec(&spec_text) else {
        return Err(invalid(interp, "")?);
    };
    if spec.align.is_some() && spec.zeropad {
        // Avoid the temptation to guess.
        return Err(invalid(
            interp,
            "; can't use explicit alignment when zero-padding",
        )?);
    }
    let ratio = slot_ratio(this, "_numerator")?;
    Ok(Value::string(format::format_fraction(
        &spec, &ratio.num, &ratio.den,
    )?))
}

// ── Method plumbing ──────────────────────────────────────────────────────────

const NEW: Sig = Sig {
    qualname: "Fraction.__new__",
    params: &["cls", "numerator", "denominator"],
    defaults: 2,
    posonly: 0,
};
const FROM_FLOAT: Sig = Sig {
    qualname: "Fraction.from_float",
    params: &["cls", "f"],
    defaults: 0,
    posonly: 0,
};
const FROM_DECIMAL: Sig = Sig {
    qualname: "Fraction.from_decimal",
    params: &["cls", "dec"],
    defaults: 0,
    posonly: 0,
};
const FROM_COPRIME_INTS: Sig = Sig {
    qualname: "Fraction._from_coprime_ints",
    params: &["cls", "numerator", "denominator"],
    defaults: 0,
    posonly: 3,
};
const LIMIT_DENOMINATOR: Sig = Sig {
    qualname: "Fraction.limit_denominator",
    params: &["self", "max_denominator"],
    defaults: 1,
    posonly: 0,
};
const FORMAT: Sig = Sig {
    qualname: "Fraction.__format__",
    params: &["self", "format_spec"],
    defaults: 0,
    posonly: 2,
};
const FORWARD: Sig = Sig {
    qualname: "Fraction._operator_fallbacks.<locals>.forward",
    params: &["a", "b"],
    defaults: 0,
    posonly: 0,
};
const REVERSE: Sig = Sig {
    qualname: "Fraction._operator_fallbacks.<locals>.reverse",
    params: &["b", "a"],
    defaults: 0,
    posonly: 0,
};
const INT: Sig = Sig {
    qualname: "Fraction.__int__",
    params: &["a", "_index"],
    defaults: 1,
    posonly: 0,
};
const ROUND: Sig = Sig {
    qualname: "Fraction.__round__",
    params: &["self", "ndigits"],
    defaults: 1,
    posonly: 0,
};
const RICHCMP: Sig = Sig {
    qualname: "Fraction._richcmp",
    params: &["self", "other", "op"],
    defaults: 0,
    posonly: 0,
};
const DEEPCOPY: Sig = Sig {
    qualname: "Fraction.__deepcopy__",
    params: &["self", "memo"],
    defaults: 0,
    posonly: 0,
};
const HASH_ALGORITHM: Sig = Sig {
    qualname: "_hash_algorithm",
    params: &["numerator", "denominator"],
    defaults: 0,
    posonly: 0,
};
const ROUND_TO_EXPONENT: Sig = Sig {
    qualname: "_round_to_exponent",
    params: &["n", "d", "exponent", "no_neg_zero"],
    defaults: 1,
    posonly: 0,
};
const ROUND_TO_FIGURES: Sig = Sig {
    qualname: "_round_to_figures",
    params: &["n", "d", "figures"],
    defaults: 0,
    posonly: 0,
};

/// A required bound parameter.
fn req<'a>(bound: &args::Bound<'a>, i: usize) -> &'a Value {
    bound[i].expect("bind() checked required parameters")
}

/// `def m(self)`.
const SELF: &[&str] = &["self"];
/// `def m(a)` (the unary operators).
const A: &[&str] = &["a"];

/// Bind a one-parameter method.
fn bind_self<'a>(
    qualname: &'static str,
    params: &'static [&'static str],
    args: &'a [ExpandedCallArg],
) -> Result<&'a Value> {
    if let [this] = args
        && this.name.is_none()
    {
        return Ok(&this.value);
    }
    let sig = Sig {
        qualname,
        params,
        defaults: 0,
        posonly: 0,
    };
    let bound = bind(&sig, args)?;
    Ok(req(&bound, 0))
}

/// Bind a two-parameter method.
fn bind_pair<'a>(sig: &Sig, args: &'a [ExpandedCallArg]) -> Result<(&'a Value, &'a Value)> {
    // Fast path for the interpreter's own operator dispatch.
    if let [x, y] = args
        && x.name.is_none()
        && y.name.is_none()
    {
        return Ok((&x.value, &y.value));
    }
    let bound = bind(sig, args)?;
    Ok((req(&bound, 0), req(&bound, 1)))
}

fn forward_method(
    interp: &mut Interpreter,
    op: ArithOp,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (a, b) = bind_pair(&FORWARD, args)?;
    forward(interp, op, a, b)
}

fn reverse_method(
    interp: &mut Interpreter,
    op: ArithOp,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (b, a) = bind_pair(&REVERSE, args)?;
    reverse(interp, op, b, a)
}

/// `_add(a, b)` and the other exposed monomorphic helpers.
fn monomorphic_method(
    qualname: &'static str,
    op: ArithOp,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let sig = Sig {
        qualname,
        params: &["a", "b"],
        defaults: 0,
        posonly: 0,
    };
    let (a, b) = bind_pair(&sig, args)?;
    let ra = slot_ratio(a, "_numerator")?;
    let rb = slot_ratio(b, "_numerator")?;
    monomorphic(op, &ra, &rb)
}

fn compare_method(
    interp: &mut Interpreter,
    qualname: &'static str,
    op: CmpOp,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let sig = Sig {
        qualname,
        params: &["a", "b"],
        defaults: 0,
        posonly: 0,
    };
    let (a, b) = bind_pair(&sig, args)?;
    richcmp(interp, a, b, op)
}

fn unary_method(
    qualname: &'static str,
    args: &[ExpandedCallArg],
    f: impl FnOnce(&Ratio) -> Result<Value>,
) -> Result<Value> {
    let a = bind_self(qualname, A, args)?;
    let ratio = slot_ratio(a, "_numerator")?;
    f(&ratio)
}

/// `__copy__` / `__deepcopy__`: an exact `Fraction` is its own clone.
fn copy_value(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    if is_exact_fraction(this) {
        return Ok(this.clone());
    }
    let ratio = slot_ratio(this, "_numerator")?;
    let cls = interp.get_attr(this, "__class__")?;
    call(interp, cls, &[int_ref(&ratio.num), int_ref(&ratio.den)])
}

/// The `_numerator` / `_denominator` slot setters.
fn set_slot(interp: &mut Interpreter, args: &[ExpandedCallArg], numerator: bool) -> Result<Value> {
    let slot = if numerator {
        "_numerator"
    } else {
        "_denominator"
    };
    let [this, value] = args else {
        return Err(type_error(format!("{slot} setter expects 2 arguments")));
    };
    let ratio = slot_ratio(&this.value, slot)?;
    let part = index_int(interp, &value.value)?;
    let updated = if numerator {
        Ratio::new(part, ratio.den.clone())
    } else {
        Ratio::new(ratio.num.clone(), part)
    };
    set_native_state(&this.value, FractionState(updated))?;
    Ok(Value::none())
}

fn get_slot(args: &[ExpandedCallArg], numerator: bool) -> Result<Value> {
    let slot = if numerator {
        "_numerator"
    } else {
        "_denominator"
    };
    let Some(this) = args.first() else {
        return Err(type_error(format!("{slot} getter expects 1 argument")));
    };
    let ratio = slot_ratio(&this.value, slot)?;
    Ok(int_ref(if numerator { &ratio.num } else { &ratio.den }))
}

/// Move `order`'s names (when present) to the end of `attrs`, in order.
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

/// Post-import hook: make `Fraction` a `numbers.Rational` ABC subclass and
/// restore CPython's namespace order.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let numbers = interp.load_module("numbers")?;
    let rational = interp.get_attr(&numbers, "Rational")?;
    let abc = interp.load_module("abc")?;
    let abc_meta = interp.get_attr(&abc, "ABCMeta")?;
    let math = interp.load_module("math")?;
    let operator = interp.load_module("operator")?;
    let sys = interp.load_module("sys")?;
    let (ValueKind::PyClass(rational), ValueKind::PyClass(abc_meta)) =
        (rational.kind(), abc_meta.kind())
    else {
        return Err(PyError::runtime(
            "fractions: numbers.Rational / abc.ABCMeta are not classes".to_string(),
        ));
    };
    let fraction = match module.borrow().attrs.get("Fraction").map(Value::kind) {
        Some(ValueKind::PyClass(class)) => Rc::clone(class),
        _ => return Err(PyError::runtime("fractions: missing Fraction".to_string())),
    };
    link_class_bases(&fraction, vec![Rc::clone(rational)]);
    {
        let mut class = fraction.borrow_mut();
        class.metatype = Some(Rc::clone(abc_meta));
        class.slots = Some(
            ["_numerator", "_denominator"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        class.attrs.insert(
            "__abstractmethods__".to_string(),
            pyrust_builtins::frozenset::frozenset(PySet::default()),
        );
        class.attrs.insert(
            "_abc_impl".to_string(),
            crate::builtin_modules::abc::new_abc_impl(),
        );
        reorder(&mut class.attrs, CLASS_ORDER);
        class.bump_mutation_version();
    }
    FRACTION_CLASSES.with(|classes| classes.borrow_mut().push(Rc::clone(&fraction)));

    let mut module = module.borrow_mut();
    for (name, value) in [
        ("math", math),
        ("numbers", numbers),
        ("operator", operator),
        ("sys", sys),
    ] {
        module.attrs.insert(name.to_string(), value);
    }
    reorder(&mut module.attrs, MODULE_ORDER);
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => Value::list(vec![Value::string("Fraction")]),
        "_PyHASH_MODULUS" => Value::int(arith::HASH_MODULUS as i64),
        "_PyHASH_INF" => Value::int(arith::HASH_INF),
    }

    /// `_hash_algorithm(numerator, denominator)`.
    fn _hash_algorithm(args) -> Result<Value> {
        let bound = bind(&HASH_ALGORITHM, args)?;
        let numerator = index_int(_interp, req(&bound, 0))?;
        let denominator = index_int(_interp, req(&bound, 1))?;
        Ok(Value::int(arith::hash_ratio(&numerator, &denominator)))
    }

    /// `_round_to_exponent(n, d, exponent, no_neg_zero=False)`.
    fn _round_to_exponent(args) -> Result<Value> {
        let bound = bind(&ROUND_TO_EXPONENT, args)?;
        let n = index_int(_interp, req(&bound, 0))?;
        let d = index_int(_interp, req(&bound, 1))?;
        let exponent = index_int(_interp, req(&bound, 2))?;
        let no_neg_zero = match bound[3] {
            Some(flag) => _interp.truthy_value(flag)?,
            None => false,
        };
        let (sign, significand) = arith::round_to_exponent(&n, &d, &exponent, no_neg_zero)?;
        Ok(Value::tuple(vec![Value::bool_(sign), int_value(significand)]))
    }

    /// `_round_to_figures(n, d, figures)`.
    fn _round_to_figures(args) -> Result<Value> {
        let bound = bind(&ROUND_TO_FIGURES, args)?;
        let n = index_int(_interp, req(&bound, 0))?;
        let d = index_int(_interp, req(&bound, 1))?;
        let figures = index_int(_interp, req(&bound, 2))?;
        let (sign, significand, exponent) = arith::round_to_figures(&n, &d, &figures)?;
        Ok(Value::tuple(vec![
            Value::bool_(sign),
            int_value(significand),
            int_value(exponent),
        ]))
    }

    class Fraction {
        constants {
            "__doc__" => Value::string(FRACTION_DOC),
            "__slots__" => Value::tuple(vec![
                Value::string("_numerator"),
                Value::string("_denominator"),
            ]),
        }

        /// Constructs a Rational.
        fn __new__(args) -> Result<Value> {
            let bound = bind(&NEW, args)?;
            let cls = fraction_subtype(req(&bound, 0))?;
            construct(_interp, &cls, bound[1], bound[2])
        }

        /// Converts a finite float to a rational number, exactly.
        #[classmethod]
        fn from_float(args) -> Result<Value> {
            let bound = bind(&FROM_FLOAT, args)?;
            from_float(_interp, req(&bound, 0), req(&bound, 1))
        }

        /// Converts a finite Decimal instance to a rational number, exactly.
        #[classmethod]
        fn from_decimal(args) -> Result<Value> {
            let bound = bind(&FROM_DECIMAL, args)?;
            from_decimal(_interp, req(&bound, 0), req(&bound, 1))
        }

        /// Convert a pair of ints to a rational number, for internal use.
        #[classmethod]
        fn _from_coprime_ints(args) -> Result<Value> {
            let bound = bind(&FROM_COPRIME_INTS, args)?;
            let cls = fraction_subtype(req(&bound, 0))?;
            let num = index_int(_interp, req(&bound, 1))?;
            let den = index_int(_interp, req(&bound, 2))?;
            new_instance(&cls, num, den)
        }

        /// Return True if the Fraction is an integer.
        fn is_integer(args) -> Result<Value> {
            let this = bind_self("Fraction.is_integer", SELF, args)?;
            Ok(Value::bool_(slot_ratio(this, "_denominator")?.den.is_one()))
        }

        /// Return a pair of integers, whose ratio is equal to the original
        /// Fraction.
        fn as_integer_ratio(args) -> Result<Value> {
            let this = bind_self("Fraction.as_integer_ratio", SELF, args)?;
            let ratio = slot_ratio(this, "_numerator")?;
            Ok(Value::tuple(vec![int_ref(&ratio.num), int_ref(&ratio.den)]))
        }

        /// Closest Fraction to self with denominator at most max_denominator.
        fn limit_denominator(args) -> Result<Value> {
            let bound = bind(&LIMIT_DENOMINATOR, args)?;
            let default = Value::int(1_000_000);
            let max_d = bound[1].unwrap_or(&default);
            limit_denominator(_interp, req(&bound, 0), max_d)
        }

        #[getter]
        fn numerator(args) -> Result<Value> {
            get_slot(args, true)
        }

        #[getter]
        fn denominator(args) -> Result<Value> {
            get_slot(args, false)
        }

        /// repr(self)
        fn __repr__(args) -> Result<Value> {
            let this = bind_self("Fraction.__repr__", SELF, args)?;
            let name = type_name(this);
            let ratio = slot_ratio(this, "_numerator")?;
            Ok(Value::string(format!(
                "{name}({}, {})",
                int_str(&ratio.num)?,
                int_str(&ratio.den)?
            )))
        }

        /// str(self)
        fn __str__(args) -> Result<Value> {
            let this = bind_self("Fraction.__str__", SELF, args)?;
            let ratio = slot_ratio(this, "_denominator")?;
            Ok(Value::string(ratio_str(&ratio)?))
        }

        /// Format this fraction according to the given format specification.
        fn __format__(args) -> Result<Value> {
            let bound = bind(&FORMAT, args)?;
            format_value(_interp, req(&bound, 0), req(&bound, 1))
        }

        /// a + b
        fn _add(args) -> Result<Value> {
            monomorphic_method("Fraction._add", ArithOp::Add, args)
        }

        /// a + b
        fn __add__(args) -> Result<Value> {
            forward_method(_interp, ArithOp::Add, args)
        }

        /// a + b
        fn __radd__(args) -> Result<Value> {
            reverse_method(_interp, ArithOp::Add, args)
        }

        /// a - b
        fn _sub(args) -> Result<Value> {
            monomorphic_method("Fraction._sub", ArithOp::Sub, args)
        }

        /// a - b
        fn __sub__(args) -> Result<Value> {
            forward_method(_interp, ArithOp::Sub, args)
        }

        /// a - b
        fn __rsub__(args) -> Result<Value> {
            reverse_method(_interp, ArithOp::Sub, args)
        }

        /// a * b
        fn _mul(args) -> Result<Value> {
            monomorphic_method("Fraction._mul", ArithOp::Mul, args)
        }

        /// a * b
        fn __mul__(args) -> Result<Value> {
            forward_method(_interp, ArithOp::Mul, args)
        }

        /// a * b
        fn __rmul__(args) -> Result<Value> {
            reverse_method(_interp, ArithOp::Mul, args)
        }

        /// a / b
        fn _div(args) -> Result<Value> {
            monomorphic_method("Fraction._div", ArithOp::Div, args)
        }

        /// a / b
        fn __truediv__(args) -> Result<Value> {
            forward_method(_interp, ArithOp::Div, args)
        }

        /// a / b
        fn __rtruediv__(args) -> Result<Value> {
            reverse_method(_interp, ArithOp::Div, args)
        }

        /// a // b
        fn _floordiv(args) -> Result<Value> {
            monomorphic_method("Fraction._floordiv", ArithOp::FloorDiv, args)
        }

        /// a // b
        fn __floordiv__(args) -> Result<Value> {
            forward_method(_interp, ArithOp::FloorDiv, args)
        }

        /// a // b
        fn __rfloordiv__(args) -> Result<Value> {
            reverse_method(_interp, ArithOp::FloorDiv, args)
        }

        /// (a // b, a % b)
        fn _divmod(args) -> Result<Value> {
            monomorphic_method("Fraction._divmod", ArithOp::DivMod, args)
        }

        /// (a // b, a % b)
        fn __divmod__(args) -> Result<Value> {
            forward_method(_interp, ArithOp::DivMod, args)
        }

        /// (a // b, a % b)
        fn __rdivmod__(args) -> Result<Value> {
            reverse_method(_interp, ArithOp::DivMod, args)
        }

        /// a % b
        fn _mod(args) -> Result<Value> {
            monomorphic_method("Fraction._mod", ArithOp::Mod, args)
        }

        /// a % b
        fn __mod__(args) -> Result<Value> {
            forward_method(_interp, ArithOp::Mod, args)
        }

        /// a % b
        fn __rmod__(args) -> Result<Value> {
            reverse_method(_interp, ArithOp::Mod, args)
        }

        /// a ** b
        fn __pow__(args) -> Result<Value> {
            let sig = Sig {
                qualname: "Fraction.__pow__",
                params: &["a", "b"],
                defaults: 0,
                posonly: 0,
            };
            let (a, b) = bind_pair(&sig, args)?;
            power(_interp, a, b)
        }

        /// a ** b
        fn __rpow__(args) -> Result<Value> {
            let sig = Sig {
                qualname: "Fraction.__rpow__",
                params: &["b", "a"],
                defaults: 0,
                posonly: 0,
            };
            let (b, a) = bind_pair(&sig, args)?;
            reflected_power(_interp, b, a)
        }

        /// +a: Coerces a subclass instance to Fraction
        fn __pos__(args) -> Result<Value> {
            unary_method("Fraction.__pos__", args, |r| new_fraction(r.num.clone(), r.den.clone()))
        }

        /// -a
        fn __neg__(args) -> Result<Value> {
            unary_method("Fraction.__neg__", args, |r| new_fraction(-&r.num, r.den.clone()))
        }

        /// abs(a)
        fn __abs__(args) -> Result<Value> {
            unary_method("Fraction.__abs__", args, |r| new_fraction(r.num.abs(), r.den.clone()))
        }

        /// int(a)
        fn __int__(args) -> Result<Value> {
            let bound = bind(&INT, args)?;
            let ratio = slot_ratio(req(&bound, 0), "_numerator")?;
            let truncated = int_value(truncate(&ratio)?);
            match bound[1] {
                Some(index) => call(_interp, index.clone(), &[truncated]),
                None => Ok(truncated),
            }
        }

        /// math.trunc(a)
        fn __trunc__(args) -> Result<Value> {
            unary_method("Fraction.__trunc__", args, |r| Ok(int_value(truncate(r)?)))
        }

        /// math.floor(a)
        fn __floor__(args) -> Result<Value> {
            unary_method("Fraction.__floor__", args, |r| {
                Ok(int_value(floor_div(&r.num, &r.den)?))
            })
        }

        /// math.ceil(a)
        fn __ceil__(args) -> Result<Value> {
            // The negations cleverly convince floordiv to return the ceiling.
            unary_method("Fraction.__ceil__", args, |r| {
                Ok(int_value(-floor_div(&-&r.num, &r.den)?))
            })
        }

        /// round(self, ndigits)
        fn __round__(args) -> Result<Value> {
            let bound = bind(&ROUND, args)?;
            let this = req(&bound, 0);
            match bound[1].filter(|nd| !nd.is_none()) {
                None => {
                    let ratio = slot_ratio(this, "_denominator")?;
                    Ok(int_value(round_half_even(&ratio)?))
                }
                Some(ndigits) => round_digits(_interp, this, ndigits),
            }
        }

        /// hash(self)
        fn __hash__(args) -> Result<Value> {
            let this = bind_self("Fraction.__hash__", SELF, args)?;
            let ratio = slot_ratio(this, "_numerator")?;
            Ok(Value::int(arith::hash_ratio(&ratio.num, &ratio.den)))
        }

        /// a == b
        fn __eq__(args) -> Result<Value> {
            let sig = Sig {
                qualname: "Fraction.__eq__",
                params: &["a", "b"],
                defaults: 0,
                posonly: 0,
            };
            let (a, b) = bind_pair(&sig, args)?;
            equals(_interp, a, b)
        }

        /// Helper for comparison operators, for internal use only.
        fn _richcmp(args) -> Result<Value> {
            let bound = bind(&RICHCMP, args)?;
            richcmp_with(_interp, req(&bound, 0), req(&bound, 1), req(&bound, 2))
        }

        /// a < b
        fn __lt__(args) -> Result<Value> {
            compare_method(_interp, "Fraction.__lt__", CmpOp::Lt, args)
        }

        /// a > b
        fn __gt__(args) -> Result<Value> {
            compare_method(_interp, "Fraction.__gt__", CmpOp::Gt, args)
        }

        /// a <= b
        fn __le__(args) -> Result<Value> {
            compare_method(_interp, "Fraction.__le__", CmpOp::Le, args)
        }

        /// a >= b
        fn __ge__(args) -> Result<Value> {
            compare_method(_interp, "Fraction.__ge__", CmpOp::Ge, args)
        }

        /// a != 0
        fn __bool__(args) -> Result<Value> {
            unary_method("Fraction.__bool__", args, |r| Ok(Value::bool_(!r.num.is_zero())))
        }

        fn __reduce__(args) -> Result<Value> {
            let this = bind_self("Fraction.__reduce__", SELF, args)?;
            let cls = _interp.get_attr(this, "__class__")?;
            let ratio = slot_ratio(this, "_numerator")?;
            Ok(Value::tuple(vec![
                cls,
                Value::tuple(vec![int_ref(&ratio.num), int_ref(&ratio.den)]),
            ]))
        }

        fn __copy__(args) -> Result<Value> {
            let this = bind_self("Fraction.__copy__", SELF, args)?;
            copy_value(_interp, this)
        }

        fn __deepcopy__(args) -> Result<Value> {
            let bound = bind(&DEEPCOPY, args)?;
            copy_value(_interp, req(&bound, 0))
        }

        #[getter("_numerator")]
        fn get_numerator_slot(args) -> Result<Value> {
            get_slot(args, true)
        }

        #[setter("_numerator")]
        fn set_numerator_slot(args) -> Result<Value> {
            set_slot(_interp, args, true)
        }

        #[getter("_denominator")]
        fn get_denominator_slot(args) -> Result<Value> {
            get_slot(args, false)
        }

        #[setter("_denominator")]
        fn set_denominator_slot(args) -> Result<Value> {
            set_slot(_interp, args, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_ratio_matches_as_integer_ratio() {
        assert_eq!(float_ratio(0.5), (BigInt::from(1), BigInt::from(2)));
        assert_eq!(float_ratio(-2.25), (BigInt::from(-9), BigInt::from(4)));
        assert_eq!(float_ratio(3.0), (BigInt::from(3), BigInt::from(1)));
        let (n, d) = float_ratio(0.1);
        assert_eq!(n, BigInt::from(3_602_879_701_896_397_u64));
        assert_eq!(d, BigInt::one() << 55);
        let (n, d) = float_ratio(f64::MIN_POSITIVE / 4.0);
        assert_eq!(n, BigInt::one());
        assert_eq!(d, BigInt::one() << 1024);
    }

    #[test]
    fn normalize_divides_out_gcd_and_sign() {
        let (n, d) = normalize(BigInt::from(10), BigInt::from(-8)).unwrap();
        assert_eq!((n, d), (BigInt::from(-5), BigInt::from(4)));
        assert!(normalize(BigInt::from(1), BigInt::zero()).is_err());
    }

    #[test]
    fn hash_matches_cpython() {
        // hash(Fraction(1, 2)) == hash(0.5) == 2**60
        assert_eq!(
            arith::hash_ratio(&BigInt::from(1), &BigInt::from(2)),
            1_i64 << 60
        );
        assert_eq!(arith::hash_ratio(&BigInt::from(-1), &BigInt::from(1)), -2);
        let modulus = BigInt::from(arith::HASH_MODULUS);
        assert_eq!(arith::hash_ratio(&BigInt::from(1), &modulus), 314_159);
        assert_eq!(arith::hash_ratio(&BigInt::from(-1), &modulus), -314_159);
        assert_eq!(
            arith::hash_ratio(&BigInt::from((1_u64 << 61) - 2), &BigInt::from(3)),
            768_614_336_404_564_650
        );
    }

    #[test]
    fn gcd_handles_mixed_sizes() {
        let big = BigInt::from(6) * (BigInt::one() << 100);
        assert_eq!(gcd(&big, &BigInt::from(-4)), BigInt::from(4));
        assert_eq!(gcd(&BigInt::zero(), &BigInt::zero()), BigInt::zero());
        assert_eq!(gcd(&BigInt::from(12), &BigInt::from(18)), BigInt::from(6));
    }

    #[test]
    fn rational_scanner_follows_the_regex() {
        let lit = parse::match_rational(" -1_000.5_0e+1_0 ").unwrap();
        assert!(lit.negative);
        assert_eq!(lit.num, "1_000");
        assert_eq!(lit.decimal.as_deref(), Some("5_0"));
        assert_eq!(lit.exp.as_deref(), Some("+1_0"));
        let lit = parse::match_rational("3 / 4").unwrap();
        assert_eq!(lit.denom.as_deref(), Some("4"));
        let lit = parse::match_rational("\u{0661}\u{0662}").unwrap();
        assert_eq!(lit.num, "12");
        for bad in [
            "", ".", "1_", "_1", "1__0", "1/", "/2", "1e", "1.d", "3/4.5", ".5/2", "1 .5", "+-1",
            "1/2/3", "0x10", "inf",
        ] {
            assert!(parse::match_rational(bad).is_none(), "{bad:?} matched");
        }
        assert!(parse::match_rational("1.").is_some());
        assert!(parse::match_rational(".5").is_some());
        assert!(parse::match_rational("1.e5").is_some());
    }

    #[test]
    fn format_spec_matcher_follows_the_regex() {
        let spec = format::parse_spec("*^+z#010,.3f").unwrap();
        assert_eq!(spec.fill, Some("*"));
        assert_eq!(spec.align, Some(b'^'));
        assert_eq!(spec.sign, Some(b'+'));
        assert!(spec.no_neg_zero && spec.alternate && spec.zeropad);
        assert_eq!(spec.minimumwidth, Some("10"));
        assert_eq!(spec.thousands_sep, Some(b','));
        assert_eq!(spec.precision, Some("3"));
        let spec = format::parse_spec("0f").unwrap();
        assert!(!spec.zeropad);
        assert_eq!(spec.minimumwidth, Some("0"));
        let spec = format::parse_spec("=e").unwrap();
        assert_eq!(spec.align, Some(b'='));
        assert!(spec.fill.is_none());
        for bad in [
            "", "d", "05", ".f", ".01f", "005f", "10.5", "<<<f", "\u{e9}f",
        ] {
            assert!(format::parse_spec(bad).is_none(), "{bad:?} matched");
        }
    }
}
