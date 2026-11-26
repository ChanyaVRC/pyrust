// `numbers` module — 100% native-Rust Python 3.12 implementation (issues
// #3147, #3371); there is no Python-source injection.
//
// CPython 3.12's `numbers` is a pure-Python module defining the PEP 3141
// numeric tower of abstract base classes (`Number`, `Complex`, `Real`,
// `Rational`, `Integral`) using `abc.ABCMeta` and `abc.abstractmethod`, with
// virtual subclass registrations for `complex`, `float`, and `int` (which also
// covers `bool`).
//
// `pyrust_module!` declares the five classes (bases, `__slots__`, and every
// default mixin method) natively.  `post_load_init` then makes them genuine
// ABCs the way `importlib.abc` does: it installs `ABCMeta` as their metaclass,
// wraps each abstract method/property in an `__isabstractmethod__` descriptor
// (a native method object cannot carry attributes), asks
// `abc.update_abstractmethods` for `__abstractmethods__`, and performs the
// `Complex.register(complex)` / `Real.register(float)` /
// `Integral.register(int)` registrations.
//
// Reference: <https://docs.python.org/3/library/numbers.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use pyrust_derive::pyrust_module;

use crate::ast::{BinaryOp, UnaryOp};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, PySet, Value, ValueKind};

/// State of an abstract-method descriptor: the wrapped native function, the
/// bound-method class to build on instance access, and the metadata a Python
/// function would expose.
struct AbstractMethodState {
    func: Value,
    bound_cls: Value,
    name: String,
    qualname: String,
    doc: Value,
}

struct BoundAbstractMethodState {
    func: Value,
    instance: Value,
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_with_args_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &IndexMap<String, Value>,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some(k.clone()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

/// `raise NotImplementedError` — the body of every abstract method.
fn not_implemented<T>() -> Result<T> {
    Err(PyError::named("NotImplementedError", String::new()))
}

/// `+value`.
fn unary_pos(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    interp.eval_unary(value.clone(), UnaryOp::Pos)
}

/// `int(value)`.
fn to_int(interp: &mut Interpreter, value: Value) -> Result<Value> {
    call_pos(interp, Value::builtin_function("int"), &[value])
}

/// `float(value)`.
fn to_float(interp: &mut Interpreter, value: Value) -> Result<Value> {
    call_pos(interp, Value::builtin_function("float"), &[value])
}

/// `abc.update_abstractmethods`-style bookkeeping for one class of the tower.
struct TowerClass {
    name: &'static str,
    /// Abstract methods defined in this class's own namespace.
    abstract_methods: &'static [&'static str],
    /// Abstract properties defined in this class's own namespace.
    abstract_properties: &'static [&'static str],
}

const TOWER: [TowerClass; 5] = [
    TowerClass {
        name: "Number",
        abstract_methods: &[],
        abstract_properties: &[],
    },
    TowerClass {
        name: "Complex",
        abstract_methods: &[
            "__complex__",
            "__add__",
            "__radd__",
            "__neg__",
            "__pos__",
            "__mul__",
            "__rmul__",
            "__truediv__",
            "__rtruediv__",
            "__pow__",
            "__rpow__",
            "__abs__",
            "conjugate",
            "__eq__",
        ],
        abstract_properties: &["real", "imag"],
    },
    TowerClass {
        name: "Real",
        abstract_methods: &[
            "__float__",
            "__trunc__",
            "__floor__",
            "__ceil__",
            "__round__",
            "__floordiv__",
            "__rfloordiv__",
            "__mod__",
            "__rmod__",
            "__lt__",
            "__le__",
        ],
        abstract_properties: &[],
    },
    TowerClass {
        name: "Rational",
        abstract_methods: &[],
        abstract_properties: &["numerator", "denominator"],
    },
    TowerClass {
        name: "Integral",
        abstract_methods: &[
            "__int__",
            "__pow__",
            "__lshift__",
            "__rlshift__",
            "__rshift__",
            "__rrshift__",
            "__and__",
            "__rand__",
            "__xor__",
            "__rxor__",
            "__or__",
            "__ror__",
            "__invert__",
        ],
        abstract_properties: &[],
    },
];

/// `(abc class, concrete builtin)` virtual subclass registrations.
const REGISTRATIONS: [(&str, &str); 3] = [
    ("Complex", "complex"),
    ("Real", "float"),
    ("Integral", "int"),
];

/// Names `numbers` re-exports from `abc`, in CPython's namespace order.
const ABC_IMPORTS: [&str; 2] = ["ABCMeta", "abstractmethod"];

/// Wrap `owner.name` (a native function) in an abstract-method descriptor.
fn wrap_abstract(
    interp: &mut Interpreter,
    abstract_cls: &Value,
    bound_cls: &Value,
    owner: &str,
    name: &str,
    func: Value,
) -> Result<Value> {
    let doc = interp
        .get_attr(&func, "__doc__")
        .unwrap_or_else(|_| Value::none());
    call_pos(
        interp,
        abstract_cls.clone(),
        &[
            func,
            bound_cls.clone(),
            Value::string(name),
            Value::string(format!("{owner}.{name}")),
            doc,
        ],
    )
}

/// Turn the natively declared tower into real ABCs and register the builtin
/// numeric types.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let abc_mod = interp.load_module("abc")?;
    let abc_meta_val = interp.get_attr(&abc_mod, "ABCMeta")?;
    let abstractmethod_val = interp.get_attr(&abc_mod, "abstractmethod")?;
    let update_abstractmethods = interp.get_attr(&abc_mod, "update_abstractmethods")?;
    let ValueKind::PyClass(abc_meta) = abc_meta_val.kind() else {
        return Err(PyError::runtime(
            "numbers: abc.ABCMeta is not a class".into(),
        ));
    };

    let take = |name: &str| -> Result<Value> {
        module
            .borrow()
            .attrs
            .get(name)
            .cloned()
            .ok_or_else(|| PyError::runtime(format!("numbers: missing {name}")))
    };
    let abstract_cls = take("_AbstractMethod")?;
    let bound_cls = take("_BoundAbstractMethod")?;

    for spec in &TOWER {
        let cls_val = take(spec.name)?;
        let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
            return Err(PyError::runtime(format!(
                "numbers: {} is not a class",
                spec.name
            )));
        };
        {
            let mut cls = cls_rc.borrow_mut();
            cls.metatype = Some(Rc::clone(abc_meta));
            // `__slots__ = ()`: the tower carries no per-instance `__dict__`.
            cls.slots = Some(IndexSet::new());
            cls.attrs.insert(
                "__abstractmethods__".to_string(),
                pyrust_builtins::frozenset::frozenset(PySet::default()),
            );
            cls.attrs.insert(
                "_abc_impl".to_string(),
                crate::builtin_modules::abc::new_abc_impl(),
            );
        }
        for &method in spec.abstract_methods {
            let func = cls_rc.borrow().attrs.get(method).cloned();
            let Some(func) = func else { continue };
            let wrapped =
                wrap_abstract(interp, &abstract_cls, &bound_cls, spec.name, method, func)?;
            cls_rc
                .borrow_mut()
                .attrs
                .insert(method.to_string(), wrapped);
        }
        for &prop in spec.abstract_properties {
            let orig = cls_rc.borrow().attrs.get(prop).cloned();
            let Some(orig) = orig else { continue };
            let fget = interp.get_attr(&orig, "fget")?;
            let wrapped = wrap_abstract(interp, &abstract_cls, &bound_cls, spec.name, prop, fget)?;
            let property =
                pyrust_builtins::property::property(wrapped, Value::none(), Value::none());
            cls_rc.borrow_mut().attrs.insert(prop.to_string(), property);
        }
        call_pos(interp, update_abstractmethods.clone(), &[cls_val])?;
    }

    // `Complex.register(complex)`, `Real.register(float)`, `Integral.register(int)`.
    let builtins = interp.load_module("builtins")?;
    for (abc_name, builtin_name) in REGISTRATIONS {
        let abc_cls = take(abc_name)?;
        let register = interp.get_attr(&abc_cls, "register")?;
        let concrete = interp.get_attr(&builtins, builtin_name)?;
        call_pos(interp, register, &[concrete])?;
    }

    // Rebuild the namespace in CPython's order — `from abc import ABCMeta,
    // abstractmethod`, then `__all__`, then the classes — and drop the private
    // descriptor helpers.
    let mut module = module.borrow_mut();
    let all = module.attrs.shift_remove("__all__");
    let mut classes = Vec::new();
    for spec in &TOWER {
        if let Some(cls) = module.attrs.shift_remove(spec.name) {
            classes.push((spec.name, cls));
        }
    }
    module.attrs.shift_remove("_AbstractMethod");
    module.attrs.shift_remove("_BoundAbstractMethod");
    module
        .attrs
        .insert(ABC_IMPORTS[0].to_string(), abc_meta_val);
    module
        .attrs
        .insert(ABC_IMPORTS[1].to_string(), abstractmethod_val);
    if let Some(all) = all {
        module.attrs.insert("__all__".to_string(), all);
    }
    for (name, cls) in classes {
        module.attrs.insert(name.to_string(), cls);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(
            "Abstract Base Classes (ABCs) for numbers, according to PEP 3141.\n\n\
             TODO: Fill out more detailed documentation on the operators."
        ),
        "__all__" => Value::list(vec![
            Value::string("Number"),
            Value::string("Complex"),
            Value::string("Real"),
            Value::string("Rational"),
            Value::string("Integral"),
        ]),
    }

    #[native_state(AbstractMethodState)]
    class _AbstractMethod {
        constants {
            "__isabstractmethod__" => Value::bool_(true),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
            bound_cls: PyValue,
            name: PyValue,
            qualname: PyValue,
            doc: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                AbstractMethodState {
                    func: func.0,
                    bound_cls: bound_cls.0,
                    name: name.0.as_str().unwrap_or_default().to_string(),
                    qualname: qualname.0.as_str().unwrap_or_default().to_string(),
                    doc: doc.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __get__(
            #[positional_only] self_val: PyValueRef<'_>,
            obj: PyValue,
            #[default(OptionalArg::Omitted)] _objtype: OptValueRef<'_>,
        ) -> Result<Value> {
            if obj.0.is_none() {
                return Ok(self_val.0.clone());
            }
            let (func, bound_cls) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.bound_cls.clone())))?;
            call_pos(_interp, bound_cls, &[func, obj.0])
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let func = with_state(self_val.0, |st| Ok(st.func.clone()))?;
            call_with_args_kwargs(_interp, func, args, &kwargs)
        }

        #[getter]
        fn __name__(#[state] st: &AbstractMethodState) -> Result<Value> {
            Ok(Value::string(&st.name))
        }

        #[getter]
        fn __qualname__(#[state] st: &AbstractMethodState) -> Result<Value> {
            Ok(Value::string(&st.qualname))
        }

        #[getter]
        fn __doc__(#[state] st: &AbstractMethodState) -> Result<Value> {
            Ok(st.doc.clone())
        }
    }

    #[native_state(BoundAbstractMethodState)]
    class _BoundAbstractMethod {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
            instance: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                BoundAbstractMethodState {
                    func: func.0,
                    instance: instance.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let (func, inst) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.instance.clone())))?;
            let mut full_pos = Vec::with_capacity(args.len() + 1);
            full_pos.push(inst);
            full_pos.extend_from_slice(args);
            call_with_args_kwargs(_interp, func, &full_pos, &kwargs)
        }
    }

    class Number {
        constants {
            "__doc__" => Value::string(
                "All numbers inherit from this class.\n\n    \
                 If you just want to check if an argument x is a number, without\n    \
                 caring what kind, use isinstance(x, Number).\n    "
            ),
            "__slots__" => Value::tuple(Vec::new()),
            // Concrete numeric types must provide their own hash implementation.
            "__hash__" => Value::none(),
        }
    }

    class Complex("Number") {
        constants {
            "__doc__" => Value::string(
                "Complex defines the operations that work on the builtin complex type.\n\n    \
                 In short, those are: a conversion to complex, .real, .imag, +, -,\n    \
                 *, /, **, abs(), .conjugate, ==, and !=.\n\n    \
                 If it is given heterogeneous arguments, and doesn't have special\n    \
                 knowledge about them, it should fall back to the builtin complex\n    \
                 type as described below.\n    "
            ),
            "__slots__" => Value::tuple(Vec::new()),
            // Defining `__eq__` without `__hash__` makes the class unhashable.
            "__hash__" => Value::none(),
        }

        /// Return a builtin complex instance. Called for complex(self).
        fn __complex__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        /// True if self != 0. Called for bool(self).
        fn __bool__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.eval_binary(self_val.0.clone(), BinaryOp::Ne, Value::int(0))
        }

        /// Retrieve the real component of this number.
        ///
        /// This should subclass Real.
        #[getter]
        fn real(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// Retrieve the imaginary component of this number.
        ///
        /// This should subclass Real.
        #[getter]
        fn imag(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// self + other
        #[allow(unused_variables)]
        fn __add__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other + self
        #[allow(unused_variables)]
        fn __radd__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// -self
        fn __neg__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// +self
        fn __pos__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// self - other
        fn __sub__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let negated = _interp.eval_unary(other.0, UnaryOp::Neg)?;
            _interp.eval_binary(self_val.0.clone(), BinaryOp::Add, negated)
        }

        /// other - self
        fn __rsub__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let negated = _interp.eval_unary(self_val.0.clone(), UnaryOp::Neg)?;
            _interp.eval_binary(negated, BinaryOp::Add, other.0)
        }

        /// self * other
        #[allow(unused_variables)]
        fn __mul__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other * self
        #[allow(unused_variables)]
        fn __rmul__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self / other: Should promote to float when necessary.
        #[allow(unused_variables)]
        fn __truediv__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other / self
        #[allow(unused_variables)]
        fn __rtruediv__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self ** exponent; should promote to float or complex when necessary.
        #[allow(unused_variables)]
        fn __pow__(#[positional_only] _self: PyValueRef<'_>, exponent: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// base ** self
        #[allow(unused_variables)]
        fn __rpow__(#[positional_only] _self: PyValueRef<'_>, base: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// Returns the Real distance from 0. Called for abs(self).
        fn __abs__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// (x+y*i).conjugate() returns (x-y*i).
        fn conjugate(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// self == other
        #[allow(unused_variables)]
        fn __eq__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }
    }

    class Real("Complex") {
        constants {
            "__doc__" => Value::string(
                "To Complex, Real adds the operations that work on real numbers.\n\n    \
                 In short, those are: a conversion to float, trunc(), divmod,\n    \
                 %, <, <=, >, and >=.\n\n    \
                 Real also provides defaults for the derived operations.\n    "
            ),
            "__slots__" => Value::tuple(Vec::new()),
        }

        /// Any Real can be converted to a native float object.
        ///
        /// Called for float(self).
        fn __float__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// trunc(self): Truncates self to an Integral.
        fn __trunc__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// Finds the greatest Integral <= self.
        fn __floor__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// Finds the least Integral >= self.
        fn __ceil__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// Rounds self to ndigits decimal places, defaulting to 0.
        ///
        /// If ndigits is omitted or None, returns an Integral, otherwise
        /// returns a Real. Rounds half toward even.
        #[allow(unused_variables)]
        fn __round__(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(None)] ndigits: Option<PyValue>,
        ) -> Result<Value> {
            not_implemented()
        }

        /// divmod(self, other): The pair (self // other, self % other).
        fn __divmod__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let quotient =
                _interp.eval_binary(self_val.0.clone(), BinaryOp::FloorDiv, other.0.clone())?;
            let remainder = _interp.eval_binary(self_val.0.clone(), BinaryOp::Mod, other.0)?;
            Ok(Value::tuple(vec![quotient, remainder]))
        }

        /// divmod(other, self): The pair (other // self, other % self).
        fn __rdivmod__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let quotient =
                _interp.eval_binary(other.0.clone(), BinaryOp::FloorDiv, self_val.0.clone())?;
            let remainder = _interp.eval_binary(other.0, BinaryOp::Mod, self_val.0.clone())?;
            Ok(Value::tuple(vec![quotient, remainder]))
        }

        /// self // other: The floor() of self/other.
        #[allow(unused_variables)]
        fn __floordiv__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other // self: The floor() of other/self.
        #[allow(unused_variables)]
        fn __rfloordiv__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self % other
        #[allow(unused_variables)]
        fn __mod__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other % self
        #[allow(unused_variables)]
        fn __rmod__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self < other
        ///
        /// < on Reals defines a total ordering, except perhaps for NaN.
        #[allow(unused_variables)]
        fn __lt__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self <= other
        #[allow(unused_variables)]
        fn __le__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// complex(self) == complex(float(self), 0)
        fn __complex__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let as_float = to_float(_interp, self_val.0.clone())?;
            call_pos(_interp, Value::builtin_function("complex"), &[as_float])
        }

        /// Real numbers are their real component.
        #[getter]
        fn real(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            unary_pos(_interp, self_val.0)
        }

        /// Real numbers have no imaginary component.
        #[getter]
        fn imag(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(0))
        }

        /// Conjugate is a no-op for Reals.
        fn conjugate(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            unary_pos(_interp, self_val.0)
        }
    }

    class Rational("Real") {
        constants {
            "__doc__" => Value::string(
                ".numerator and .denominator should be in lowest terms."
            ),
            "__slots__" => Value::tuple(Vec::new()),
        }

        /// The numerator of a rational number in lowest terms.
        #[getter]
        fn numerator(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// The denominator of a rational number in lowest terms.
        ///
        /// This denominator should be positive.
        #[getter]
        fn denominator(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// float(self) = self.numerator / self.denominator
        ///
        /// It's important that this conversion use the integer's "true"
        /// division rather than casting one side to float before dividing
        /// so that ratios of huge integers convert without overflowing.
        fn __float__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let numerator = _interp.get_attr(self_val.0, "numerator")?;
            let numerator = to_int(_interp, numerator)?;
            let denominator = _interp.get_attr(self_val.0, "denominator")?;
            let denominator = to_int(_interp, denominator)?;
            _interp.eval_binary(numerator, BinaryOp::Div, denominator)
        }
    }

    class Integral("Rational") {
        constants {
            "__doc__" => Value::string(
                "Integral adds methods that work on integral numbers.\n\n    \
                 In short, these are conversion to int, pow with modulus, and the\n    \
                 bit-string operations.\n    "
            ),
            "__slots__" => Value::tuple(Vec::new()),
        }

        /// int(self)
        fn __int__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// Called whenever an index is needed, such as in slicing
        fn __index__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            to_int(_interp, self_val.0.clone())
        }

        /// self ** exponent % modulus, but maybe faster.
        #[allow(unused_variables)]
        fn __pow__(
            #[positional_only] _self: PyValueRef<'_>,
            exponent: PyValue,
            #[default(None)] modulus: Option<PyValue>,
        ) -> Result<Value> {
            not_implemented()
        }

        /// self << other
        #[allow(unused_variables)]
        fn __lshift__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other << self
        #[allow(unused_variables)]
        fn __rlshift__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self >> other
        #[allow(unused_variables)]
        fn __rshift__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other >> self
        #[allow(unused_variables)]
        fn __rrshift__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self & other
        #[allow(unused_variables)]
        fn __and__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other & self
        #[allow(unused_variables)]
        fn __rand__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self ^ other
        #[allow(unused_variables)]
        fn __xor__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other ^ self
        #[allow(unused_variables)]
        fn __rxor__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// self | other
        #[allow(unused_variables)]
        fn __or__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// other | self
        #[allow(unused_variables)]
        fn __ror__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            not_implemented()
        }

        /// ~self
        fn __invert__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            not_implemented()
        }

        /// float(self) == float(int(self))
        fn __float__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let as_int = to_int(_interp, self_val.0.clone())?;
            to_float(_interp, as_int)
        }

        /// Integers are their own numerators.
        #[getter]
        fn numerator(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            unary_pos(_interp, self_val.0)
        }

        /// Integers have a denominator of 1.
        #[getter]
        fn denominator(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(1))
        }
    }
}
