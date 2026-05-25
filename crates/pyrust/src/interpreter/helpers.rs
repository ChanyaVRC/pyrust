/// `a ** b` for non-negative integer exponent, promoting to `BigInt` if the
/// result would overflow `i64`.  Matches CPython's arbitrary-precision int
/// semantics — `2 ** 64` returns the BigInt `18446744073709551616`, not the
/// wrapped value `0`.  The fast path is a single `checked_pow` (≈free), so
/// non-overflowing call sites pay no measurable cost over `wrapping_pow`.
///
/// Centralised here (issue #421 / PR #484 Copilot review) so the `**`
/// operator and the `pow(a, b)` builtin share one source of truth.
pub(crate) fn int_pow_promoting(a: i64, b: i64) -> Value {
    debug_assert!(b >= 0, "int_pow_promoting: caller must guard b < 0");
    let exp = match u32::try_from(b) {
        Ok(e) => e,
        // Exponent doesn't fit in u32 — `a == 0` or `a == 1` or `a == -1` are
        // the only finite-result cases; everything else is astronomically
        // large.  Promote unconditionally; BigInt::pow handles the trivial
        // bases cheaply and produces an honest BigInt for the rest.
        Err(_) => {
            return Value::bigint(PyPow::pow(PyBigInt::from(a), b as u64));
        }
    };
    match a.checked_pow(exp) {
        Some(r) => Value::int(r),
        None => Value::bigint(PyPow::pow(PyBigInt::from(a), exp)),
    }
}
/// Returns the Python type name string for a `Value`, used in error messages.
///
/// Thin alias for [`pyrust_core::builtin_type_name`] — kept locally so the
/// many interpreter call sites stay short.  Returns `Cow<'static, str>` so
/// `PyInstance` can report its runtime class name without a leak; static
/// names stay zero-allocation.
pub(crate) fn value_type_name_str(v: &Value) -> std::borrow::Cow<'static, str> {
    pyrust_core::builtin_type_name(v)
}

/// Exact ordering between an `i64` integer and an `f64` float.
///
/// Mirrors CPython's richcmp for int vs float: instead of converting the int
/// to f64 (lossy beyond 2^53), we convert the float to its exact integer value
/// and compare there.  Handles all finite and non-finite floats:
///
/// - NaN: returns `None` (caller must treat as unordered; the `compare`
///   wrapper in `expr.rs` already short-circuits NaN to `false`).
/// - `±inf`: ordered relative to every finite i64.
/// - Integer-valued finite float: compare `(f as i64)` to `i`.
/// - Fractional finite float: `i` equals `f.trunc() as i64` only if the
///   fractional part pushes `f` strictly away — positive fraction means
///   `f > i`, negative fraction means `f < i`.
/// - Out-of-i64-range finite float: sign decides ordering.
fn int_float_cmp(i: i64, f: f64) -> Option<std::cmp::Ordering> {
    if f.is_nan() {
        return None;
    }
    // f is ±inf or finite.
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64; // 2^63
    if f >= I64_MAX_PLUS_ONE {
        // float is larger than every i64
        return Some(std::cmp::Ordering::Less);
    }
    if f < (i64::MIN as f64) {
        // float is smaller than every i64
        return Some(std::cmp::Ordering::Greater);
    }
    // f is finite and in [i64::MIN, 2^63); safe to cast.
    let trunc = f.trunc();
    let trunc_i = trunc as i64;
    // base = how i compares to trunc_i (the integer value of f rounded toward zero).
    let base = i.cmp(&trunc_i);
    if base != std::cmp::Ordering::Equal || f == trunc {
        // i != trunc_i: the ordering is unambiguous.
        // i == trunc_i and f is integer-valued: exact equality.
        Some(base)
    } else {
        // i == trunc_i but f has a fractional part: f lies strictly between
        // two integers.  Positive fraction: trunc_i < f < trunc_i+1, so i < f.
        // Negative fraction: trunc_i-1 < f < trunc_i, so i > f.
        if f > 0.0 {
            Some(std::cmp::Ordering::Less)
        } else {
            Some(std::cmp::Ordering::Greater)
        }
    }
}

/// Exact ordering between a `BigInt` and an `f64` float.
///
/// Uses `BigInt::from_f64` (returns `None` for NaN/infinity; for fractional
/// finite floats it truncates toward zero rather than returning `None`) for
/// the integer-valued case — guarded by `f == f.trunc()` so fractional
/// floats fall through to the heuristic path below.  For out-of-range or
/// non-integer floats, falls back to a sign + magnitude heuristic that
/// mirrors CPython's implementation.
fn bigint_float_cmp(big: &crate::value::PyBigInt, f: f64) -> Option<std::cmp::Ordering> {
    use crate::value::PyBigInt;
    use num_traits::FromPrimitive;
    if f.is_nan() {
        return None;
    }
    // For integer-valued finite floats: convert to BigInt and compare exactly.
    if f.is_finite() && f == f.trunc() {
        return PyBigInt::from_f64(f).map(|fi| big.cmp(&fi));
    }
    if f.is_infinite() {
        return if f > 0.0 {
            Some(std::cmp::Ordering::Less) // big < +inf
        } else {
            Some(std::cmp::Ordering::Greater) // big > -inf
        };
    }
    // Fractional finite float: compare big to f.trunc() and adjust.
    let trunc = f.trunc();
    let base = PyBigInt::from_f64(trunc)
        .map(|ti| big.cmp(&ti))
        .unwrap_or(std::cmp::Ordering::Less);
    if base != std::cmp::Ordering::Equal {
        return Some(base);
    }
    // big == trunc but f has a fractional part.
    if f > 0.0 {
        Some(std::cmp::Ordering::Less) // big < f
    } else {
        Some(std::cmp::Ordering::Greater) // big > f
    }
}

/// Total order for Python values used by `sorted()` / `min()` / `max()` and
/// comparison operators.  Mirrors CPython's `<` semantics: numbers by
/// magnitude, strings lexicographically, bools as 0/1, lists and tuples
/// lexicographically element-by-element.  Incomparable pairs return a
/// `TypeError`.
pub(crate) fn compare_values(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
    compare_values_with_op(a, b, "<")
}

/// Like `compare_values` but uses `op_name` in the `TypeError` message when
/// the operand types are incompatible.  CPython's `do_richcompare` emits the
/// operator token that was actually requested (`<`, `>`, `<=`, `>=`), so
/// `eval_binary` calls this variant directly for `Gt`, `Le`, and `Ge`.
pub(crate) fn compare_values_with_op(
    a: &Value,
    b: &Value,
    op_name: &str,
) -> Result<std::cmp::Ordering> {
    use crate::value::PyBigInt;
    match (a.kind(), b.kind()) {
        (ValueKind::Int(x), ValueKind::Int(y)) => Ok(x.cmp(&y)),
        (ValueKind::Float(x), ValueKind::Float(y)) => Ok(x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)),
        // Use exact integer comparison to avoid precision loss beyond 2^53.
        // int_float_cmp converts the float to an exact integer value rather
        // than widening the int to f64.  NaN falls through to Equal (the
        // `compare` wrapper in expr.rs pre-filters NaN via `is_nan` checks).
        (ValueKind::Int(x), ValueKind::Float(y)) => Ok(int_float_cmp(x, y).unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Float(x), ValueKind::Int(y)) => Ok(int_float_cmp(y, x).map(|o| o.reverse()).unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Bool(x), ValueKind::Bool(y)) => Ok(x.cmp(&y)),
        (ValueKind::Bool(x), ValueKind::Int(y)) => Ok((x as i64).cmp(&y)),
        (ValueKind::Int(x), ValueKind::Bool(y)) => Ok(x.cmp(&(y as i64))),
        (ValueKind::BigInt(x), ValueKind::BigInt(y)) => Ok(x.cmp(y)),
        (ValueKind::BigInt(x), ValueKind::Int(y)) => Ok((*x).cmp(&PyBigInt::from(y))),
        (ValueKind::Int(x), ValueKind::BigInt(y)) => Ok(PyBigInt::from(x).cmp(y)),
        (ValueKind::BigInt(x), ValueKind::Float(y)) => {
            Ok(bigint_float_cmp(x, y).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::Float(x), ValueKind::BigInt(y)) => Ok(bigint_float_cmp(y, x)
            .map(|o| o.reverse())
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Str(x), ValueKind::Str(y)) => Ok(x.cmp(y)),
        (ValueKind::List(x), ValueKind::List(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                let ord = compare_values_with_op(a, b, op_name)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        (ValueKind::Tuple(x), ValueKind::Tuple(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                let ord = compare_values_with_op(a, b, op_name)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{op_name}' not supported between instances of '{}' and '{}'",
                value_type_name_str(a),
                value_type_name_str(b),
            ),
        )),
    }
}

pub(crate) fn lookup_class_attr(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    let (value, base) = {
        let borrowed = class.borrow();
        (borrowed.attrs.get(name).cloned(), borrowed.base.clone())
    };
    if value.is_some() {
        return value;
    }
    base.and_then(|base| lookup_class_attr(&base, name))
}

thread_local! {
    static OBJECT_CLASS: Rc<RefCell<PyClass>> = Rc::new(RefCell::new(PyClass {
        name: "object".to_string(),
        qualname: "object".to_string(),
        base: None,
        attrs: IndexMap::new(),
        mutation_version: std::cell::Cell::new(0),
    }));

    /// Per-primitive `PyClass` singletons.  Issue #462 — `int`, `str`,
    /// `list`, … are now real `PyClass` values, not `BuiltinFunction(name)`
    /// sentinels.  `type(x)` returns the matching entry here; the names
    /// resolve to these classes via `resolve_builtin`; and `isinstance`
    /// works through the standard `class_is_subclass_of` walk.
    ///
    /// Each class's `__init__` is the existing `BuiltinFunction("<name>")`
    /// constructor (so `int("42")` etc. keep their established behaviour);
    /// the call-site dispatch in `call_class_expanded` recognises primitive
    /// classes and returns the constructor's `Value` directly instead of
    /// wrapping it in a `PyInstance`.
    ///
    /// `bool` chains its `base` to `int`, matching CPython's
    /// `bool.__bases__ == (int,)`.  Storage-variant constraints prevent
    /// subclassing primitives in pyrust today; the migration is purely
    /// metadata + dispatch routing.
    static PRIMITIVE_CLASSES: PrimitiveClasses = build_primitive_classes();

    /// O(1) dispatch table for primitive classes (#462 perf): maps the
    /// `Rc<RefCell<PyClass>>` identity (by raw pointer) to the registry's
    /// `BuiltinDispatchFn` for the corresponding constructor.  Populated
    /// once per thread alongside `PRIMITIVE_CLASSES`.
    ///
    /// Hot path: `call_function_expanded`'s `ValueKind::PyClass(class)`
    /// arm looks up `Rc::as_ptr(class)` here; on hit it dispatches
    /// directly to the registry fn, skipping `call_class_expanded`'s
    /// PyInstance allocation + `lookup_class_attr("__init__")` walk +
    /// recursive `call_function_expanded` step.  The lookup is one
    /// `HashMap::get` (cap = 11) + a fn pointer call.
    static PRIMITIVE_CLASS_DISPATCH:
        std::cell::RefCell<
            std::collections::HashMap<
                *const std::cell::RefCell<PyClass>,
                crate::builtin_registry::BuiltinDispatchFn,
            >,
        > = {
        let cell = std::cell::RefCell::new(std::collections::HashMap::with_capacity(11));
        PRIMITIVE_CLASSES.with(|c| {
            let mut m = cell.borrow_mut();
            for (class, name) in [
                (&c.bool_class, "bool"),
                (&c.bytes_class, "bytes"),
                (&c.complex_class, "complex"),
                (&c.dict_class, "dict"),
                (&c.float_class, "float"),
                (&c.frozenset_class, "frozenset"),
                (&c.int_class, "int"),
                (&c.list_class, "list"),
                (&c.set_class, "set"),
                (&c.str_class, "str"),
                (&c.tuple_class, "tuple"),
            ] {
                if let Some(dispatch) = crate::builtin_registry::lookup(name) {
                    m.insert(Rc::as_ptr(class), dispatch);
                }
            }
        });
        cell
    };
}

/// Holder for the per-primitive `PyClass` Rc's.  Constructed once per
/// thread at startup, then cloned cheaply (Rc::clone) on every `type(x)` /
/// `resolve_builtin("int")` etc. call.
pub(crate) struct PrimitiveClasses {
    pub(crate) bool_class: Rc<RefCell<PyClass>>,
    pub(crate) bytes_class: Rc<RefCell<PyClass>>,
    pub(crate) complex_class: Rc<RefCell<PyClass>>,
    pub(crate) dict_class: Rc<RefCell<PyClass>>,
    pub(crate) float_class: Rc<RefCell<PyClass>>,
    pub(crate) frozenset_class: Rc<RefCell<PyClass>>,
    pub(crate) int_class: Rc<RefCell<PyClass>>,
    pub(crate) list_class: Rc<RefCell<PyClass>>,
    pub(crate) mappingproxy_class: Rc<RefCell<PyClass>>,
    pub(crate) set_class: Rc<RefCell<PyClass>>,
    pub(crate) str_class: Rc<RefCell<PyClass>>,
    pub(crate) tuple_class: Rc<RefCell<PyClass>>,
}

/// Build the per-primitive `PyClass` singletons.  Called once per thread
/// (via `thread_local!` init).  Each class's `__init__` slot is the
/// existing builtin constructor (`BuiltinFunction("int")` etc.) so that
/// `T(args)` keeps its existing behaviour through `call_class_expanded`'s
/// primitive short-circuit.
///
/// `#[cold]` + `#[inline(never)]` keeps this one-time init code out of the
/// hot-path icache footprint of `call_function_expanded`, `get_attr`, etc.
/// — observable as a small but uniform speedup on benches that never touch
/// primitive classes (`literal_int`, `literal_dict`).
#[cold]
#[inline(never)]
fn build_primitive_classes() -> PrimitiveClasses {
    #[cold]
    #[inline(never)]
    fn make(name: &'static str, base: Option<Rc<RefCell<PyClass>>>) -> Rc<RefCell<PyClass>> {
let attrs: IndexMap<String, Value> = IndexMap::new();
        // Note: no `__init__` is installed.  Direct `int(5)` / `str(x)` calls
        // dispatch via `PRIMITIVE_CLASS_DISPATCH` (HashMap → registry fn),
        // bypassing `__init__` entirely.  Installing the BuiltinFunction
        // constructor as `__init__` would leak it to subclasses via
        // `lookup_class_attr`, where `invoke_class_method` prepends the
        // fresh `PyInstance` receiver and breaks the constructor signature
        // (`class S(int): pass; S(5)` → `int(PyInstance, 5)` argument
        // mismatch).  See Copilot review on #463.
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            qualname: name.to_string(),
            base,
            attrs,
            mutation_version: std::cell::Cell::new(0),
        }))
    }
    let int_class = make("int", None);
    let str_class = make("str", None);
    let list_class = make("list", None);
    let tuple_class = make("tuple", None);
    let dict_class = make("dict", None);
    let set_class = make("set", None);
    let bytes_class = make("bytes", None);
    populate_primitive_methods(&int_class, "int", INT_METHODS);
    populate_primitive_methods(&bytes_class, "bytes", BYTES_METHODS);
    populate_primitive_methods(&str_class, "str", STR_METHODS);
    populate_primitive_methods(&list_class, "list", LIST_METHODS);
    populate_primitive_methods(&tuple_class, "tuple", TUPLE_METHODS);
    populate_primitive_methods(&dict_class, "dict", DICT_METHODS);
    populate_primitive_methods(&set_class, "set", SET_METHODS);
    let complex_class = make("complex", None);
    let frozenset_class = make("frozenset", None);
    let float_class = make("float", None);
    populate_primitive_methods(&complex_class, "complex", COMPLEX_METHODS);
    populate_primitive_methods(&frozenset_class, "frozenset", FROZENSET_METHODS);
    populate_primitive_methods(&float_class, "float", FLOAT_METHODS);
    // `fromhex` is a classmethod: register it directly in float_class.attrs
    // so both `float.fromhex(s)` and `(1.0).fromhex(s)` resolve to the
    // same `BuiltinFunction("float.fromhex")` sentinel.
    float_class
        .borrow_mut()
        .attrs
        .insert("fromhex".to_string(), Value::builtin_function("float.fromhex"));
    // Issue #988: register `__init__` on dict/list/set so that
    // `super().__init__()` from a subclass can resolve it via MRO lookup
    // without raising AttributeError.  The registered dispatch returns None
    // (no-op) when called from super() with no args, and populates the
    // backing store when called via invoke_class_method with constructor args.
    for (cls, type_name) in [
        (&list_class, "list"),
        (&dict_class, "dict"),
        (&set_class, "set"),
    ] {
        let sentinel: &'static str =
            Box::leak(format!("{type_name}.__init__").into_boxed_str());
        cls.borrow_mut()
            .attrs
            .insert("__init__".to_string(), Value::builtin_function(sentinel));
    }
    // PEP 585: `__class_getitem__` on the five collection types that support
    // `list[int]`-style generic subscripting.  Each gets a
    // `BuiltinFunction("<type>.__class_getitem__")` sentinel so that both
    // `list[int]` (via `eval_index`) and `list.__class_getitem__(int)` (via
    // `call_function_expanded`) produce a `GenericAlias` value.  CPython 3.12
    // exposes `__class_getitem__` only on these five built-in types.
    for (cls, type_name) in [
        (&list_class, "list"),
        (&tuple_class, "tuple"),
        (&dict_class, "dict"),
        (&set_class, "set"),
        (&frozenset_class, "frozenset"),
    ] {
        let sentinel: &'static str =
            Box::leak(format!("{type_name}.__class_getitem__").into_boxed_str());
        cls.borrow_mut()
            .attrs
            .insert("__class_getitem__".to_string(), Value::builtin_function(sentinel));
    }
    PrimitiveClasses {
        bytes_class,
        complex_class,
        dict_class,
        float_class,
        frozenset_class,
        list_class,
        mappingproxy_class: make("mappingproxy", None),
        set_class,
        str_class,
        tuple_class,
        // `bool` inherits from `int` (CPython: `bool.__bases__ == (int,)`).
        bool_class: make("bool", Some(Rc::clone(&int_class))),
        int_class,
    }
}

/// Authoritative per-primitive method registries.  Keep each in sync with
/// the corresponding `match method` in `pyrust_builtins::<type>::call`
/// (or `Interpreter::call_<type>_method` for dict/set, which also have
/// their own `pyrust_builtins::<type>::call` fallback).  Class-attr access
/// (`list.append`) returns a `BuiltinFunction("list.append")` sentinel
/// dispatched by the unified `<type>.<method>` arm in
/// `call_function_expanded`.
const INT_METHODS: &[&str] = &["bit_length", "bit_count", "is_integer"];
const BYTES_METHODS: &[&str] = pyrust_builtins::bytes::METHODS;

const STR_METHODS: &[&str] = &[
    "index", "count",
    "split", "rsplit", "join", "splitlines", "partition", "rpartition",
    "strip", "lstrip", "rstrip", "removeprefix", "removesuffix",
    "center", "ljust", "rjust", "zfill", "expandtabs",
    "upper", "lower", "casefold", "capitalize", "swapcase", "title",
    "find", "rfind", "rindex",
    "replace", "format", "format_map",
    "startswith", "endswith",
    "isdigit", "isalpha", "isalnum", "isspace", "isdecimal", "isnumeric",
    "islower", "isupper", "istitle", "isascii", "isidentifier", "isprintable",
];

const LIST_METHODS: &[&str] = &[
    "index", "count",
    "append", "clear", "copy", "extend", "insert", "pop", "remove", "reverse",
    "sort",
];

const TUPLE_METHODS: &[&str] = &["index", "count"];

// `fromkeys` is a classmethod in CPython and isn't implemented by
// `dict::call`/`call_dict_method`; leaving it out until it lands.
const DICT_METHODS: &[&str] = &[
    "get", "keys", "values", "items", "update", "pop", "popitem", "clear",
    "setdefault", "copy",
];

const SET_METHODS: &[&str] = &[
    "add", "remove", "discard", "pop", "clear",
    "update", "intersection_update", "difference_update", "symmetric_difference_update",
    "copy", "union", "intersection", "difference", "symmetric_difference",
    "issubset", "issuperset", "isdisjoint",
];

const COMPLEX_METHODS: &[&str] = &["conjugate"];

const FROZENSET_METHODS: &[&str] = &[
    "copy", "union", "intersection", "difference", "symmetric_difference",
    "issubset", "issuperset", "isdisjoint",
];

// `fromhex` is a classmethod and is registered separately in build_primitive_classes.
const FLOAT_METHODS: &[&str] = pyrust_builtins::float::METHODS;

/// Install `BuiltinFunction("<type>.<name>")` sentinels into the class's
/// `attrs` for every name in `methods`.  Each qualified name is leaked
/// once per thread — the storage is fixed-size and permanent (one entry
/// per method per type), and `Value::builtin_function` requires
/// `&'static str`.  See [`populate_str_methods`]'s removed predecessor
/// for the prior shape; this generalised form drives str/list/tuple/dict/
/// set together.
#[cold]
#[inline(never)]
fn populate_primitive_methods(
    class: &Rc<RefCell<PyClass>>,
    type_name: &'static str,
    methods: &[&'static str],
) {
    let mut cls = class.borrow_mut();
    cls.attrs.reserve(methods.len());
    for &name in methods {
        let qualified: &'static str =
            Box::leak(format!("{type_name}.{name}").into_boxed_str());
        cls.attrs
            .insert(name.to_string(), Value::builtin_function(qualified));
    }
}

/// Returns the singleton synthetic `object` class used as the terminal
/// entry of every class's `__mro__`. pyrust does not (yet) model `object`
/// as a real first-class type — every user class chains to `None` — so
/// this provides a stable, identity-comparable terminator so that
/// `A.__mro__[-1] is B.__mro__[-1]` holds, matching CPython.
pub(crate) fn object_class_singleton() -> Rc<RefCell<PyClass>> {
    OBJECT_CLASS.with(|c| Rc::clone(c))
}

/// Look up the per-primitive `PyClass` singleton for one of the 11 migrated
/// primitive type names (`int`, `str`, `list`, …).  Returns `None` for any
/// other name — callers fall through to the legacy `BuiltinFunction(name)`
/// path.  See [`PRIMITIVE_CLASSES`].
pub(crate) fn primitive_class_by_name(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    PRIMITIVE_CLASSES.with(|c| {
        Some(Rc::clone(match name {
            "bool" => &c.bool_class,
            "bytes" => &c.bytes_class,
            "complex" => &c.complex_class,
            "dict" => &c.dict_class,
            "float" => &c.float_class,
            "frozenset" => &c.frozenset_class,
            "int" => &c.int_class,
            "list" => &c.list_class,
            "mappingproxy" => &c.mappingproxy_class,
            "set" => &c.set_class,
            "str" => &c.str_class,
            "tuple" => &c.tuple_class,
            _ => return None,
        }))
    })
}

/// Return the `PyClass` that `type(v)` should yield for any of the 11
/// migrated primitive types.  Returns `None` for variants that aren't
/// part of this migration (functions, modules, instances, …) — the
/// caller falls back to its existing per-variant logic.
pub(crate) fn primitive_class_for_value(v: &Value) -> Option<Rc<RefCell<PyClass>>> {
    let name: &'static str = match v.kind() {
        ValueKind::Bool(_) => "bool",
        ValueKind::Int(_) | ValueKind::BigInt(_) => "int",
        ValueKind::Float(_) => "float",
        ValueKind::Str(_) => "str",
        ValueKind::List(_) => "list",
        ValueKind::Tuple(_) => "tuple",
        ValueKind::Dict(_) => "dict",
        ValueKind::Set(_) => "set",
        ValueKind::Bytes(_) => "bytes",
        ValueKind::Complex(_, _) => "complex",
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "frozenset" => "frozenset",
        ValueKind::BuiltinObject { ops, .. }
            if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME =>
        {
            "mappingproxy"
        }
        _ => return None,
    };
    primitive_class_by_name(name)
}

/// True iff `class` is one of the 11 migrated-primitive class singletons.
/// O(1) via the [`PRIMITIVE_CLASS_DISPATCH`] table.
pub(crate) fn is_primitive_class(class: &Rc<RefCell<PyClass>>) -> bool {
    primitive_class_dispatch(class).is_some()
}

/// Fast-path dispatch lookup for primitive classes (#462 perf).
/// Returns the registry's `BuiltinDispatchFn` for the constructor of
/// the named primitive (`int`, `str`, …), or `None` for any other
/// class.  Called from `call_function_expanded`'s `PyClass` arm to
/// skip the `call_class_expanded` PyInstance-alloc + `__init__`-walk
/// + recursive `call_function_expanded` chain — three layers of
/// dispatch collapsed into one `HashMap` lookup and one fn-pointer
/// call.
#[inline]
pub(crate) fn primitive_class_dispatch(
    class: &Rc<RefCell<PyClass>>,
) -> Option<crate::builtin_registry::BuiltinDispatchFn> {
    let ptr = Rc::as_ptr(class);
    PRIMITIVE_CLASS_DISPATCH.with(|m| m.borrow().get(&ptr).copied())
}

/// Fast `isinstance(obj, primitive_class)` — when `cls` is one of the
/// 11 primitive class singletons, skip the `class_is_subclass_of`
/// walk (which would require materialising `obj`'s class via
/// `primitive_class_for_value`'s thread_local + Rc::clone) and do a
/// direct `ValueKind` tag check.  `Some(true/false)` on a hit,
/// `None` if `cls` isn't a primitive class — fall through to the
/// general walk.  Issue #462 perf.
#[inline]
pub(crate) fn primitive_class_isinstance_fast(
    obj: &Value,
    cls: &Rc<RefCell<PyClass>>,
) -> Option<bool> {
    // Issue #976: a PyInstance may subclass a primitive.  Skip the fast-path
    // tag check and return None so the caller falls through to the general
    // `class_is_subclass_of` MRO walk, which correctly handles
    // `isinstance(MyDict(), dict)` when MyDict inherits from dict.
    if matches!(obj.kind(), ValueKind::PyInstance(_)) {
        return None;
    }
    let cls_ptr = Rc::as_ptr(cls);
    PRIMITIVE_CLASSES.with(|c| {
        // bool ⊂ int: an int-class test matches both Int and Bool.
        // Every other primitive is a tag identity.
        if cls_ptr == Rc::as_ptr(&c.int_class) {
            return Some(matches!(
                obj.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            ));
        }
        if cls_ptr == Rc::as_ptr(&c.bool_class) {
            return Some(matches!(obj.kind(), ValueKind::Bool(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.str_class) {
            return Some(matches!(obj.kind(), ValueKind::Str(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.float_class) {
            return Some(matches!(obj.kind(), ValueKind::Float(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.list_class) {
            return Some(matches!(obj.kind(), ValueKind::List(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.tuple_class) {
            return Some(matches!(obj.kind(), ValueKind::Tuple(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.dict_class) {
            return Some(matches!(obj.kind(), ValueKind::Dict(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.set_class) {
            return Some(matches!(obj.kind(), ValueKind::Set(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.bytes_class) {
            return Some(matches!(obj.kind(), ValueKind::Bytes(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.complex_class) {
            return Some(matches!(obj.kind(), ValueKind::Complex(_, _)));
        }
        if cls_ptr == Rc::as_ptr(&c.frozenset_class) {
            return Some(matches!(
                obj.kind(),
                ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "frozenset"
            ));
        }
        if cls_ptr == Rc::as_ptr(&c.mappingproxy_class) {
            return Some(matches!(
                obj.kind(),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME
            ));
        }
        None
    })
}

/// Walk the base chain of `class` and return the name of the first
/// primitive builtin base found (`"dict"`, `"list"`, `"set"`, …), or
/// `None` if the class does not inherit from any primitive.
///
/// Only the directly-supported container primitives that need backing-
/// data storage are returned: `dict`, `list`, and `set`.  Other
/// primitives (`int`, `str`, `float`, …) require deep storage-variant
/// changes and are out of scope for issue #976.
pub(crate) fn find_mutable_primitive_base(
    class: &Rc<RefCell<PyClass>>,
) -> Option<&'static str> {
    let (name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    match name.as_str() {
        "dict" | "list" | "set" => {
            // Check that this is actually the primitive singleton, not a
            // user class that happens to be named "dict".
            if is_primitive_class(class) {
                return Some(match name.as_str() {
                    "dict" => "dict",
                    "list" => "list",
                    "set" => "set",
                    _ => unreachable!(),
                });
            }
        }
        _ => {}
    }
    base.and_then(|b| find_mutable_primitive_base(&b))
}

/// Walk the base chain of `class` and return the name of the first
/// immutable primitive builtin base found (`"frozenset"` or `"tuple"`),
/// or `None` if the class does not inherit from either.
///
/// These types are immutable — their backing must be populated from the
/// constructor argument at `__new__` time, before any `__init__` runs.
/// Unlike the mutable types handled by `find_mutable_primitive_base`,
/// there is no empty pre-initialisation step (issue #994).
pub(crate) fn find_immutable_primitive_base(
    class: &Rc<RefCell<PyClass>>,
) -> Option<&'static str> {
    let (name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    match name.as_str() {
        "frozenset" | "tuple" => {
            // Check that this is actually the primitive singleton, not a
            // user class that happens to share the name.
            if is_primitive_class(class) {
                return Some(match name.as_str() {
                    "frozenset" => "frozenset",
                    "tuple" => "tuple",
                    _ => unreachable!(),
                });
            }
        }
        _ => {}
    }
    base.and_then(|b| find_immutable_primitive_base(&b))
}

/// Constant key used to store the backing primitive value inside a
/// `PyInstance` that subclasses `dict`, `list`, or `set`.
pub(crate) const BUILTIN_DATA_ATTR: &str = "__builtin_data__";

/// Extract the backing primitive value from a `PyInstance` that was
/// constructed by `call_class_expanded` for a subclass of `dict`,
/// `list`, or `set`.  Returns `None` for any other instance.
pub(crate) fn instance_builtin_data(inst: &Rc<RefCell<PyInstance>>) -> Option<Value> {
    inst.borrow()
        .attrs
        .get(BUILTIN_DATA_ATTR)
        .cloned()
}

pub(crate) struct PrintOptions {
    pub(crate) values: Vec<Value>,
    pub(crate) sep: String,
    pub(crate) end: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExpandedCallArg {
    pub(crate) name: Option<String>,
    pub(crate) value: Value,
}

/// Invoke a method that was looked up on a class — handling both
/// `UserFunction` methods (compiled Python bytecode, bound via the
/// interpreter's user-function path) and `BuiltinFunction` methods
/// (registered Rust dispatch fns from `pyrust_module!`'s `class` block).
///
/// In both cases `instance` is prepended as the implicit `self` —
/// matching how `inst.method(...)` semantics work in CPython.  This
/// helper centralises the binding rule so dunder dispatch sites
/// (`__getitem__`, `__iter__`, `__call__`, `__len__`, `__init__`,
/// …) don't have to repeat the UserFunction-vs-BuiltinFunction
/// branching at every call site.
pub(crate) fn invoke_class_method(
    interp: &mut Interpreter,
    method_val: Value,
    instance: Value,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    match method_val.kind() {
        ValueKind::UserFunction(f) => {
            let func = Rc::clone(f);
            interp.call_user_function_expanded(func, args, &[instance])
        }
        ValueKind::BuiltinFunction(name) => {
            let dispatch = crate::builtin_registry::lookup(name).ok_or_else(|| {
                PyError::Runtime(format!(
                    "internal: builtin method '{name}' not in registry"
                ))
            })?;
            let mut combined: Vec<ExpandedCallArg> = Vec::with_capacity(args.len() + 1);
            combined.push(ExpandedCallArg {
                name: None,
                value: instance,
            });
            combined.extend(args.iter().cloned());
            dispatch(interp, &combined)
        }
        _ => {
            // Resolved class attr is something other than a function —
            // usually because the user did `Foo.method = 42` or similar.
            // Surface the class name + the offending value's type so the
            // diagnostic is actionable.
            let class_name = match instance.kind() {
                ValueKind::PyInstance(i) => i.borrow().class.borrow().name.clone(),
                _ => "<unknown>".to_string(),
            };
            Err(PyError::named(
                "TypeError",
                format!(
                    "'{class_name}' class attribute is not callable (got {})",
                    value_type_name_str(&method_val),
                ),
            ))
        }
    }
}

fn extract_optional_string(value: Value, name: &str) -> Result<Option<String>> {
    match value.kind() {
        ValueKind::Str(text) => Ok(Some(text.to_string())),
        ValueKind::None => Ok(None),
        _ => Err(PyError::Runtime(format!(
            "print() {} must be None or a string",
            name
        ))),
    }
}

pub(crate) fn reject_keyword_args_expanded(function_name: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if let Some(arg) = args.iter().find(|arg| arg.name.is_some()) {
        // CPython raises TypeError, not RuntimeError, for unexpected
        // kwargs.  Match that so user code using `except TypeError:` on
        // builtin call failures keeps working.
        let kw = arg.name.as_deref().unwrap_or("");
        return Err(PyError::named(
            "TypeError",
            format!("{function_name}() got an unexpected keyword argument '{kw}'"),
        ));
    }
    Ok(())
}

pub(crate) fn py_mod_i64(a: i64, b: i64) -> i64 {
    let mut remainder = a % b;
    if (remainder > 0 && b < 0) || (remainder < 0 && b > 0) {
        remainder += b;
    }
    remainder
}

/// CPython's Py_HASH_MODULUS = 2^61 - 1 (Mersenne prime).
///
/// Used by `py_hash_int` and `py_hash_bigint` to reduce hash values the
/// same way CPython's `long_hash` does.  Shared between `value_to_pykey`
/// (dict/set key storage) and the `hash()` builtin so both code paths stay
/// in sync (issue #503).
pub(crate) const PY_HASH_MODULUS: i64 = (1i64 << 61) - 1;

/// Hash an `i64` integer using CPython's Mersenne-prime scheme.
///
/// For values with `|v| < 2^61-1` the result equals `v`, subject to the
/// `-1 → -2` sentinel remap.  Larger values are reduced modulo `2^61-1`
/// first (matching CPython `long_hash`).
///
/// The `-1 → -2` remap is always applied: `-1` is the C-level `tp_hash`
/// error sentinel and must never be the hash of any Python object.
pub(crate) fn py_hash_int(v: i64) -> i64 {
    let raw = v % PY_HASH_MODULUS;
    if raw == -1 { -2 } else { raw }
}

/// Reduce a `BigInt` to an `i64` hash using CPython's Mersenne-prime scheme.
///
/// Algorithm mirrors CPython `long_hash`:
/// 1. `r = n % (2^61 - 1)` — sign-preserving remainder.
/// 2. If `r == -1`, remap to `-2` (sentinel exclusion).
///
/// The result is in `[-(2^61-2), 2^61-1]`, always fitting in `i64`.
pub(crate) fn py_hash_bigint(n: &PyBigInt) -> i64 {
    let modulus = PyBigInt::from(PY_HASH_MODULUS);
    let reduced = n.clone() % &modulus;
    let raw = reduced.to_i64().unwrap_or(0);
    if raw == -1 { -2 } else { raw }
}

fn normalize_index(index: &Value, len: usize, label: &str) -> Result<usize> {
    normalize_index_inner(index, len, label, &format!("{label} index out of range"))
}

fn normalize_index_write(index: &Value, len: usize, label: &str) -> Result<usize> {
    normalize_index_inner(
        index,
        len,
        label,
        &format!("{label} assignment index out of range"),
    )
}

fn normalize_index_inner(index: &Value, len: usize, label: &str, oor_msg: &str) -> Result<usize> {
    let mut value = match index.kind() {
        ValueKind::Int(v) => v,
        ValueKind::Bool(b) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{label} indices must be integers or slices, not {}",
                    value_type_name_str(index)
                ),
            ))
        }
    };
    if value < 0 {
        value += len as i64;
    }
    if value < 0 || value >= len as i64 {
        return Err(PyError::named("IndexError", oor_msg));
    }
    Ok(value as usize)
}

pub(crate) fn class_is_subclass_of(class: &Rc<RefCell<PyClass>>, expected: &Rc<RefCell<PyClass>>) -> bool {
    if Rc::ptr_eq(class, expected) {
        return true;
    }
    // The synthetic `object` class is a universal parent: every PyClass
    // (primitive or user-defined) reports it as the terminal of
    // `__mro__`.  `class_is_subclass_of(_, object)` must agree so
    // `issubclass(int, int.__bases__[0])` and `isinstance(x, object)`
    // hold — see Copilot review on #463.
    if Rc::ptr_eq(expected, &object_class_singleton()) {
        return true;
    }
    let base = class.borrow().base.clone();
    base.is_some_and(|base| class_is_subclass_of(&base, expected))
}

/// Runtime-side "is this class an exception?" predicate used by the
/// `raise`/`except` machinery.  Forwards to the canonical implementation
/// in `pyrust_core` so the runtime and `Value::repr`/`Value::str` paths
/// cannot drift apart (issue #429: divergence here let `raise
/// GeneratorExit(...)` succeed while `repr(GeneratorExit(...))` fell back
/// to the default `<X object>` formatting).
pub(crate) fn is_exception_class(class: &Rc<RefCell<PyClass>>) -> bool {
    pyrust_core::class_chain_contains_exception(class)
}

/// Walk the class base chain and return `true` if any class in the chain has
/// the given `name`.  Used to check subclass relationships by class name when
/// the `Rc` singleton for the expected class is not in scope.
fn class_chain_contains_name(class: &Rc<RefCell<PyClass>>, name: &str) -> bool {
    let (class_name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    if class_name == name {
        return true;
    }
    base.is_some_and(|base| class_chain_contains_name(&base, name))
}

pub(crate) fn instantiate_exception(class: Rc<RefCell<PyClass>>, args: Vec<Value>) -> Value {
    let mut attrs = IndexMap::new();
    // CPython 3.12: StopIteration.__init__ sets self.value = args[0] if args else None.
    // Mirror that here so `except StopIteration as e: e.value` always works.
    // Use a name-chain walk so subclasses (e.g. `class MyStop(StopIteration)`) also
    // get the `.value` attribute set — fixes #612.
    let is_stop_iteration = class_chain_contains_name(&class, "StopIteration");
    attrs.insert("args".to_string(), Value::tuple(args.clone()));
    if is_stop_iteration {
        let val = args.into_iter().next().unwrap_or_else(Value::none);
        attrs.insert("value".to_string(), val);
    }
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate an `ImportError` or `ModuleNotFoundError` with `.name` and
/// `.path` instance attributes, matching CPython 3.12 `ImportError.__init__`.
///
/// `class_name` must be `"ImportError"` or `"ModuleNotFoundError"`.
/// `message` becomes `args[0]`.
/// `module_name` is stored as `.name`; if `None`, `.name` is set to `None`.
/// `.path` is always `None` (pyrust has no physical package paths).
pub(crate) fn instantiate_import_error(
    class: Rc<RefCell<PyClass>>,
    message: String,
    module_name: Option<String>,
) -> Value {
    let mut attrs = IndexMap::new();
    attrs.insert("args".to_string(), Value::tuple(vec![Value::string(message)]));
    let name_val = match module_name {
        Some(n) => Value::string(n),
        None => Value::none(),
    };
    attrs.insert("name".to_string(), name_val);
    attrs.insert("path".to_string(), Value::none());
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Snapshot of an instance's own attrs as a fresh `Value::dict`.  Backs both
/// `obj.__dict__` (env.rs) and `vars(obj)` (builtins.rs) so the two stay in
/// lock-step.  CPython's `__dict__` is a live mapping; we return a clone —
/// mutations to the returned dict do not propagate back to the instance.
/// Tracked as a follow-up to #392 (live-dict semantics).
pub(crate) fn instance_attrs_snapshot(instance: &Rc<RefCell<PyInstance>>) -> Value {
    let mut dict: IndexMap<PyKey, Value> = IndexMap::new();
    for (k, v) in instance.borrow().attrs.iter() {
        dict.insert(PyKey::str_from(k), v.clone());
    }
    Value::dict(dict)
}

/// Ordered list of `(python_name, class_rc)` pairs for all 31 built-in
/// exception classes, built once per thread.  Both `install_exception_builtins`
/// and `ExcClasses::from_cache` clone the `Rc`s from here instead of
/// reconstructing the exception hierarchy on every `Interpreter::default()`.
type ExcClassEntry = (&'static str, Rc<RefCell<PyClass>>);

#[cold]
fn build_exc_classes() -> Vec<ExcClassEntry> {
    // CPython 3.12 hierarchy (single-inheritance model):
    //   BaseException
    //     Exception
    //       ArithmeticError → OverflowError, ZeroDivisionError, FloatingPointError
    //       LookupError → IndexError, KeyError
    //       ValueError → UnicodeError → UnicodeEncodeError / UnicodeDecodeError
    //       RuntimeError → RecursionError, NotImplementedError
    //       TypeError, NameError → UnboundLocalError
    //       AssertionError, AttributeError, StopIteration, SyntaxError
    //       MemoryError, ImportError → ModuleNotFoundError
    //       OSError → FileNotFoundError, FileExistsError
    //     SystemExit, GeneratorExit, KeyboardInterrupt (direct BaseException children)
    let mk = |name: &str, base: Option<Rc<RefCell<PyClass>>>| {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            qualname: name.to_string(),
            base,
            attrs: IndexMap::new(),
            mutation_version: std::cell::Cell::new(0),
        }))
    };
    let base_exception = mk("BaseException", None);
    let exception = mk("Exception", Some(Rc::clone(&base_exception)));
    let arithmetic_error = mk("ArithmeticError", Some(Rc::clone(&exception)));
    let lookup_error = mk("LookupError", Some(Rc::clone(&exception)));
    let runtime_error = mk("RuntimeError", Some(Rc::clone(&exception)));
    let type_error = mk("TypeError", Some(Rc::clone(&exception)));
    let value_error = mk("ValueError", Some(Rc::clone(&exception)));
    let name_error = mk("NameError", Some(Rc::clone(&exception)));
    let assertion_error = mk("AssertionError", Some(Rc::clone(&exception)));
    let stop_iteration = mk("StopIteration", Some(Rc::clone(&exception)));
    let attribute_error = mk("AttributeError", Some(Rc::clone(&exception)));
    let syntax_error = mk("SyntaxError", Some(Rc::clone(&exception)));
    let memory_error = mk("MemoryError", Some(Rc::clone(&exception)));
    let import_error = mk("ImportError", Some(Rc::clone(&exception)));
    let os_error = mk("OSError", Some(Rc::clone(&exception)));
    let overflow_error = mk("OverflowError", Some(Rc::clone(&arithmetic_error)));
    let zero_division_error = mk("ZeroDivisionError", Some(Rc::clone(&arithmetic_error)));
    let floating_point_error = mk("FloatingPointError", Some(Rc::clone(&arithmetic_error)));
    let index_error = mk("IndexError", Some(Rc::clone(&lookup_error)));
    let key_error = mk("KeyError", Some(Rc::clone(&lookup_error)));
    let recursion_error = mk("RecursionError", Some(Rc::clone(&runtime_error)));
    let not_implemented_error = mk("NotImplementedError", Some(Rc::clone(&runtime_error)));
    let unbound_local_error = mk("UnboundLocalError", Some(Rc::clone(&name_error)));
    let unicode_error = mk("UnicodeError", Some(Rc::clone(&value_error)));
    let module_not_found_error = mk("ModuleNotFoundError", Some(Rc::clone(&import_error)));
    let file_not_found_error = mk("FileNotFoundError", Some(Rc::clone(&os_error)));
    let file_exists_error = mk("FileExistsError", Some(Rc::clone(&os_error)));
    let unicode_encode_error = mk("UnicodeEncodeError", Some(Rc::clone(&unicode_error)));
    let unicode_decode_error = mk("UnicodeDecodeError", Some(Rc::clone(&unicode_error)));
    let system_exit = mk("SystemExit", Some(Rc::clone(&base_exception)));
    let generator_exit = mk("GeneratorExit", Some(Rc::clone(&base_exception)));
    let keyboard_interrupt = mk("KeyboardInterrupt", Some(Rc::clone(&base_exception)));
    vec![
        ("BaseException", base_exception),
        ("Exception", exception),
        ("ArithmeticError", arithmetic_error),
        ("OverflowError", overflow_error),
        ("ZeroDivisionError", zero_division_error),
        ("FloatingPointError", floating_point_error),
        ("LookupError", lookup_error),
        ("IndexError", index_error),
        ("KeyError", key_error),
        ("RuntimeError", runtime_error),
        ("RecursionError", recursion_error),
        ("NotImplementedError", not_implemented_error),
        ("TypeError", type_error),
        ("ValueError", value_error),
        ("NameError", name_error),
        ("UnboundLocalError", unbound_local_error),
        ("AssertionError", assertion_error),
        ("StopIteration", stop_iteration),
        ("AttributeError", attribute_error),
        ("SyntaxError", syntax_error),
        ("MemoryError", memory_error),
        ("ImportError", import_error),
        ("ModuleNotFoundError", module_not_found_error),
        ("UnicodeError", unicode_error),
        ("UnicodeEncodeError", unicode_encode_error),
        ("UnicodeDecodeError", unicode_decode_error),
        ("OSError", os_error),
        ("FileNotFoundError", file_not_found_error),
        ("FileExistsError", file_exists_error),
        ("SystemExit", system_exit),
        ("GeneratorExit", generator_exit),
        ("KeyboardInterrupt", keyboard_interrupt),
    ]
}

thread_local! {
    /// Per-thread cache of all 32 built-in exception class `Rc`s.
    /// Built once per thread; each `Interpreter::default()` call clones the
    /// `Rc`s (O(1) reference-count bumps) instead of allocating fresh
    /// `Rc<RefCell<PyClass>>` objects for the full hierarchy.
    static EXC_CLASS_CACHE: Vec<ExcClassEntry> = build_exc_classes();

    /// Per-thread cache of the `builtins` module value.
    /// `seed_module_dunders` clones this `Value` (one `Rc` increment) instead
    /// of rebuilding the module's ~136-entry `HashMap<String, Value>` from
    /// scratch on every script invocation.
    static BUILTINS_MODULE_CACHE: Value =
        crate::builtin_modules::load_builtin_module("builtins")
            .unwrap_or_else(Value::none);
}

/// Return a clone of the thread-local `builtins` module.  O(1) — clones
/// the `Rc<RefCell<PyModule>>` reference; the attrs map is shared.
pub(crate) fn cached_builtins_module() -> Value {
    BUILTINS_MODULE_CACHE.with(Value::clone)
}

/// Look up a built-in exception class by name, using the thread-local cache.
/// Called by `resolve_builtin` to service `LoadGlobal("TypeError")` etc.
/// without inserting exception classes into the module env at startup.
/// Triggers `EXC_CLASS_CACHE` initialisation on the very first call.
pub(crate) fn lookup_exc_class(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    EXC_CLASS_CACHE.with(|cache| {
        cache
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, cls)| Rc::clone(cls))
    })
}

/// Build the `ExcClasses` map from the thread-local cache.  Called once
/// per interpreter (lazily, on first exception raise or class lookup).
pub(crate) fn build_exc_class_map(
) -> std::collections::HashMap<&'static str, Rc<RefCell<PyClass>>> {
    EXC_CLASS_CACHE.with(|cache| {
        let mut map = std::collections::HashMap::with_capacity(cache.len());
        for (name, cls) in cache {
            map.insert(*name, Rc::clone(cls));
        }
        map
    })
}

pub(crate) fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(v) => Value::float(f64::from_bits(v)),
        PyKey::Str(v) => v,
        PyKey::Bool(v) => Value::bool_(v),
        PyKey::None => Value::none(),
        PyKey::FrozenSet(items) => {
            let mut set = indexmap::IndexSet::new();
            for k in items {
                set.insert(k);
            }
            pyrust_builtins::frozenset::frozenset(set)
        }
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Object { value, .. } => value,
    }
}

/// Merge the registers of a single VM frame view into `dict`,
/// dereferencing the view's raw pointer.  Used by
/// [`snapshot_current_locals`].
///
/// SAFETY: `view.regs_ptr` / `view.regs_len` describe a VM frame's
/// register slice.  `Interpreter::vm_frame_views` is pushed
/// immediately before each `run_bytecode_inner` invocation and
/// popped immediately after.  Push/pop sites:
///   * `program.rs::try_exec_vm_script_with_index` — `FrameKind::Script`
///     for the module/script body.
///   * `calls.rs::call_user_function_expanded` — `FrameKind::Function`
///     for both the simple and variadic user-function call paths.
///   * `vm.rs::resume_generator_with_exc` — `FrameKind::Function` for
///     each generator resume (the frame view's regs pointer comes
///     from the heap-allocated `GeneratorFrame::regs`, which is
///     stable across yields).
/// Class-body evaluation (`Insn::MakeClass`) publishes a `FrameKind::Class`
/// view so that `locals()` inside a class body returns the partially-built
/// class attrs dict (issue #487).
/// The slice is read-only here.
fn merge_frame_view_into_dict(
    view: &VmFrameView,
    dict: &mut indexmap::IndexMap<PyKey, Value>,
) {
    // Stable iteration order: walk the fastlocal slot table sorted by
    // slot index, mirroring the compiler's name-allocation order.
    let mut by_slot: Vec<(usize, &String)> = view
        .local_index
        .iter()
        .map(|(name, &slot)| (slot as usize, name))
        .collect();
    by_slot.sort_by_key(|(slot, _)| *slot);
    for (slot, name) in by_slot {
        if slot >= view.regs_len {
            continue;
        }
        // SAFETY: `view.regs_ptr` is a NonNull pointer to the frame's
        // register file; `slot < view.regs_len` is enforced above.
        //
        // No aliasing UB: as of PR #646, every `run_bytecode*` function
        // accepts `RegSlice` (raw pointer + len) instead of `&mut [Value]`.
        // `RegSlice` carries no LLVM `noalias` attribute, so no exclusive
        // borrow on the allocation is live when this code runs — even in
        // case (c) below where the script frame is the "current" frame.
        //
        // Cases for the frame being read:
        //   (a) A suspended outer frame (e.g. the Script frame when
        //       `locals()` / `globals()` fires from inside a nested
        //       function or class body): the outer frame's `RegSlice`
        //       is on the stack but carries no noalias.  No write races
        //       this read (interpreter is single-threaded).
        //   (b) The current innermost function/class frame — suspended
        //       inside `call_function_expanded` while the builtin runs.
        //       Same reasoning: `RegSlice`, no noalias, no concurrent
        //       writes.
        //   (c) The current Script frame when `locals()` is called at
        //       module scope.  The script frame's dispatch loop holds a
        //       `RegSlice` for the same allocation; forming `&Value` here
        //       does not alias an `&mut [Value]` and is sound.  (This
        //       was the residual UB in the previous `&mut [Value]` design,
        //       now closed by the `RegSlice` change in issue #547.)
        //
        // The `&Value` from `as_ref()` lives only for the duration of the
        // `.clone()` call and does not escape this loop body.
        let val = unsafe { view.regs_ptr.add(slot).as_ref() };
        if !val.is_unset() {
            dict.insert(PyKey::str_from(name), val.clone());
        }
    }
}

/// Take a snapshot of the innermost VM frame's local namespace
/// (issue #389: backing for `locals()`).  Reads the top of
/// `Interpreter::vm_frame_views` regardless of kind — at module scope
/// the top entry IS the `Script` frame (so `locals()` == `globals()`,
/// matching CPython parity), and inside a function it's the
/// `Function` frame.  Falls back to the current env's `values` map
/// when no frame is published (e.g. evaluating in a non-VM context).
pub(crate) fn snapshot_current_locals(
    interp: &Interpreter,
) -> indexmap::IndexMap<PyKey, Value> {
    let mut dict: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
    match interp.vm_frame_views.last() {
        Some(view) if view.kind == FrameKind::Script => {
            // Module scope: include the module env (built-in
            // classes + already-spilled bindings) so the user sees the
            // same complete view as `globals()`.
            let me = module_env(&interp.env);
            for (k, v) in me.borrow().values.iter() {
                dict.insert(PyKey::str_from(k), v.clone());
            }
            merge_frame_view_into_dict(view, &mut dict);
        }
        Some(view) if view.kind == FrameKind::Class => {
            // Class-body scope (issue #487): return the partially-built class
            // attrs dict — i.e. the fastlocal registers of the class body,
            // filtered to names that have been assigned so far.  CPython
            // returns the class namespace dict (which becomes `__dict__`).
            // We do NOT include the module env here, matching CPython:
            // `locals()` inside a class body is the class namespace, not
            // the module globals.
            merge_frame_view_into_dict(view, &mut dict);
        }
        Some(view) => {
            // Function scope: the function's own fastlocals plus any
            // nonlocal bindings.  Matches CPython — `locals()` inside a
            // function includes `nonlocal` names as they live in an
            // enclosing scope but are part of this function's logical
            // local namespace (issue #486).
            //
            // The frame view's `local_index` enumerates exactly the
            // names the compiler allocated for THIS function call, so
            // `merge_frame_view_into_dict` covers the fastlocal subset.
            // We deliberately do NOT also walk `interp.env.values`: when
            // the callee did not need its own local env (the
            // `needs_local_env == false` path in
            // `call_user_function_expanded`), `interp.env` points at
            // the function's *defining* env, and walking that would leak
            // enclosing-scope names into the snapshot.
            merge_frame_view_into_dict(view, &mut dict);
            // Nonlocal bindings: look up each name through the env chain
            // starting from the function's own local env.  Fast-path:
            // skip entirely when the function has no nonlocal names (the
            // common case, zero overhead).
            if let (Some(nonlocal_names), Some(env)) =
                (&view.nonlocal_names, &view.env)
            {
                for name in nonlocal_names.iter() {
                    // `lookup_name_in_enclosing_local_env` walks the env
                    // parent chain from `env` upward to find the first
                    // ancestor that declares `name` as a local and holds
                    // its value.  Errors here are internal inconsistencies
                    // (nonlocal declared but no enclosing binding found);
                    // ignore them silently rather than propagating — a
                    // missing nonlocal shouldn't crash `locals()`.
                    if let Ok(Some(val)) =
                        lookup_name_in_enclosing_local_env(env, name)
                    {
                        dict.insert(PyKey::str_from(name), val);
                    }
                }
            }
        }
        None => {
            // No active VM frame: fall back to env.values.
            for (k, v) in interp.env.borrow().values.iter() {
                dict.insert(PyKey::str_from(k), v.clone());
            }
        }
    }
    dict
}

pub(crate) fn module_env(env: &EnvRef) -> EnvRef {
    let mut current = Rc::clone(env);
    loop {
        let parent = current.borrow().parent.clone();
        match parent {
            Some(parent) => current = parent,
            None => return current,
        }
    }
}

pub(crate) fn lookup_name_in_module(env: &EnvRef, name: &str) -> Option<Value> {
    module_env(env).borrow().values.get(name).cloned()
        .or_else(|| lookup_exc_class(name).map(Value::py_class))
}

/// Sync all module-env values into `module_globals_dict` and set
/// `globals_accessed = true`.  Called by `globals()` and `locals()` at
/// module scope so the returned live dict is fully up to date.
///
/// After this call, `assign_name` will also mirror every new assignment
/// into the dict, keeping it live for the rest of execution.
pub(crate) fn sync_module_env_to_globals_dict(interp: &mut Interpreter) {
    interp.globals_accessed = true;
    // Sync env.values (dunders, names stored via StoreGlobal or assign_name).
    let me = module_env(&interp.env);
    let pairs: Vec<(String, Value)> = me.borrow().values.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, v) in pairs {
        let _ = interp.module_globals_dict.dict_insert(PyKey::str_from(&k), v);
    }
    // Also sync fastlocal registers from the active script frame (issue #820).
    // With fastlocal mode restored, module-scope assignments write only to the
    // register, not to env.values — so the dict would be stale without this.
    // SAFETY: `script_view.regs_ptr` points to the script frame's register
    // file (a SmallVec/Vec on the stack of `try_exec_vm_script_with_index`).
    // The pointer is valid because that function pushes the VmFrameView before
    // calling `run_bytecode` and pops it afterwards; we are called from within
    // `run_bytecode` (via globals()/locals()), so the stack frame is still live.
    // We read each slot as a shared reference (no &mut) and clone the Value.
    if let Some(script_view) = interp
        .vm_frame_views
        .iter()
        .find(|v| v.kind == FrameKind::Script)
    {
        let slots: Vec<(String, usize)> = script_view
            .local_index
            .iter()
            .map(|(name, &slot)| (name.clone(), slot as usize))
            .collect();
        for (name, slot) in slots {
            if slot < script_view.regs_len {
                // SAFETY: slot < regs_len, and the register file lives for the
                // duration of the script dispatch loop (see above comment).
                let val = unsafe { script_view.regs_ptr.add(slot).as_ref() }.clone();
                if !val.is_unset() {
                    let _ = interp.module_globals_dict.dict_insert(PyKey::str_from(&name), val);
                }
            }
        }
    }
}

fn has_local_binding_in_current_or_ancestor(env: &EnvRef, name: &str) -> bool {
    let mut current = Some(Rc::clone(env));
    while let Some(candidate) = current {
        let (is_function_scope, has_name, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed.local_names.contains(name),
                borrowed.parent.clone(),
            )
        };
        if is_function_scope && has_name {
            return true;
        }
        current = next;
    }
    false
}

fn find_enclosing_local_env_for_name(env: &EnvRef, name: &str) -> Option<EnvRef> {
    let mut current = env.borrow().parent.clone();
    while let Some(candidate) = current {
        let (is_function_scope, has_name, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed.local_names.contains(name),
                borrowed.parent.clone(),
            )
        };
        if is_function_scope && has_name {
            return Some(candidate);
        }
        current = next;
    }
    None
}

fn lookup_name_in_enclosing_local_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    let Some(target_env) = find_enclosing_local_env_for_name(env, name) else {
        return Err(PyError::Runtime(format!(
            "no binding for nonlocal '{}' found",
            name
        )));
    };
    lookup_name_in_env(&target_env, name)
}


// Write `value` into `env` for `name`.
#[inline]
fn env_assign_local(env: &EnvRef, name: &str, value: Value) {
    env.borrow_mut().values.insert(name.to_string(), value);
}

fn lookup_name_in_env(env: &EnvRef, name: &str) -> Result<Option<Value>> {
    let borrowed = env.borrow();
    let value = borrowed.values.get(name).cloned();
    let is_local_name = borrowed.local_names.contains(name);
    let parent = borrowed.parent.clone();
    drop(borrowed);
    if value.is_some() {
        return Ok(value);
    }
    if is_local_name {
        return Err(PyError::named(
            "UnboundLocalError",
            format!(
                "cannot access local variable '{}' where it is not associated with a value",
                name
            ),
        ));
    }
    match parent {
        Some(parent) => lookup_name_in_env(&parent, name),
        None => Ok(None),
    }
}

pub(crate) fn collect_local_names(
    params: &[crate::ast::FunctionParam],
    body: &[Stmt],
    global_names: &HashSet<String>,
    nonlocal_names: &HashSet<String>,
) -> indexmap::IndexSet<String> {
    let mut names: indexmap::IndexSet<String> =
        params.iter().map(|param| param.name.clone()).collect();
    collect_local_names_from_block(body, &mut names, global_names, nonlocal_names);
    names
}

fn collect_local_names_from_block(
    body: &[Stmt],
    names: &mut indexmap::IndexSet<String>,
    global_names: &HashSet<String>,
    nonlocal_names: &HashSet<String>,
) {
    for stmt in body {
        match stmt {
            Stmt::Assign(target, _) => {
                collect_assign_target_names(target, names, global_names, nonlocal_names);
            }
            Stmt::AttrAssign { .. } => {}
            Stmt::Def { name, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
            }
            Stmt::Class { name, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
            }
            Stmt::Global(_) | Stmt::Nonlocal(_) => {}
            Stmt::Import {
                names: import_names,
            } => {
                for (module, alias) in import_names {
                    let bound = alias
                        .clone()
                        .unwrap_or_else(|| module.split('.').next().unwrap_or(module).to_string());
                    if !global_names.contains(&bound) && !nonlocal_names.contains(&bound) {
                        names.insert(bound);
                    }
                }
            }
            Stmt::ImportFrom {
                names: import_names,
                ..
            } => {
                for (attr_name, alias) in import_names {
                    if attr_name == "*" {
                        continue;
                    }
                    let bound = alias.clone().unwrap_or_else(|| attr_name.clone());
                    if !global_names.contains(&bound) && !nonlocal_names.contains(&bound) {
                        names.insert(bound);
                    }
                }
            }
            Stmt::AnnAssign { name, .. } => {
                // Both `x: T = v` (value = Some) and `x: T` (value = None) declare
                // a local slot.  At function scope the bare form causes UnboundLocalError
                // on read (matching CPython); at class scope the slot is allocated but
                // never stored via RecordClassStore so it does not appear in vars(C).
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
            }
            Stmt::AugAssign { .. }
            | Stmt::IndexAssign { .. }
            | Stmt::SliceAssign { .. }
            | Stmt::Delete(_)
            | Stmt::Raise { .. }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Pass => {}
            // Walk expressions for walrus operator targets.
            Stmt::Expr(e) => {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
            Stmt::Return(Some(e)) => {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
            Stmt::Return(None) => {}
            Stmt::Assert { test, msg } => {
                collect_walrus_targets_in_expr(test, names, global_names, nonlocal_names);
                if let Some(m) = msg {
                    collect_walrus_targets_in_expr(m, names, global_names, nonlocal_names);
                }
            }
            Stmt::With { items, body } => {
                for (_, alias) in items {
                    if let Some(target) = alias {
                        collect_assign_target_names(target, names, global_names, nonlocal_names);
                    }
                }
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (cond, branch) in branches {
                    collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::While {
                cond,
                body,
                else_branch,
            } => {
                collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::Try {
                body,
                handlers,
                else_branch,
                finally_branch,
            } => {
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                for handler in handlers {
                    if let Some(name) = &handler.name
                        && !global_names.contains(name) && !nonlocal_names.contains(name) {
                            names.insert(name.clone());
                        }
                    collect_local_names_from_block(
                        &handler.body,
                        names,
                        global_names,
                        nonlocal_names,
                    );
                }
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
                if let Some(branch) = finally_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
            } => {
                collect_walrus_targets_in_expr(iter, names, global_names, nonlocal_names);
                collect_assign_target_names(target, names, global_names, nonlocal_names);
                collect_local_names_from_block(body, names, global_names, nonlocal_names);
                if let Some(branch) = else_branch {
                    collect_local_names_from_block(branch, names, global_names, nonlocal_names);
                }
            }
            Stmt::Match { subject, arms } => {
                collect_walrus_targets_in_expr(subject, names, global_names, nonlocal_names);
                for arm in arms {
                    // Collect capture names introduced by patterns.
                    collect_pattern_names(&arm.pattern, names, global_names, nonlocal_names);
                    if let Some(guard) = &arm.guard {
                        collect_walrus_targets_in_expr(guard, names, global_names, nonlocal_names);
                    }
                    collect_local_names_from_block(&arm.body, names, global_names, nonlocal_names);
                }
            }
        }
    }
}

/// Collect names that a pattern binds (capture patterns, star captures in sequences,
/// and `**rest` in mappings).
fn collect_pattern_names(
    pattern: &crate::ast::Pattern,
    names: &mut indexmap::IndexSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    use crate::ast::Pattern;
    match pattern {
        Pattern::Wildcard | Pattern::Literal(_) => {}
        Pattern::Capture(name) => {
            if !global_names.contains(name) && !nonlocal_names.contains(name) {
                names.insert(name.clone());
            }
        }
        Pattern::Or(alts) => {
            for alt in alts {
                collect_pattern_names(alt, names, global_names, nonlocal_names);
            }
        }
        Pattern::Sequence(elems) => {
            for (elem_pat, _) in elems {
                collect_pattern_names(elem_pat, names, global_names, nonlocal_names);
            }
        }
        Pattern::Mapping(pairs, rest) => {
            for (_, val_pat) in pairs {
                collect_pattern_names(val_pat, names, global_names, nonlocal_names);
            }
            if let Some(rest_name) = rest
                && !global_names.contains(rest_name) && !nonlocal_names.contains(rest_name) {
                    names.insert(rest_name.clone());
                }
        }
        Pattern::Class { kwargs, .. } => {
            for (_, attr_pat) in kwargs {
                collect_pattern_names(attr_pat, names, global_names, nonlocal_names);
            }
        }
    }
}

pub(crate) fn collect_global_names(body: &[Stmt]) -> HashSet<String> {
    collect_declared_names(body, |s| {
        if let Stmt::Global(names) = s { Some(names) } else { None }
    })
}

pub(crate) fn collect_nonlocal_names(body: &[Stmt]) -> HashSet<String> {
    collect_declared_names(body, |s| {
        if let Stmt::Nonlocal(names) = s { Some(names) } else { None }
    })
}

/// Collect the names used as annotation targets (`x: T` or `x: T = v`) in the
/// direct body, without descending into nested `Def` or `Class` scopes.  Used
/// by `compile_def` to detect conflicts between annotated names and
/// `global`/`nonlocal` declarations (CPython raises `SyntaxError` for these).
pub(crate) fn collect_annotation_target_names(body: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_annotation_target_names_from_block(body, &mut names);
    names
}

fn collect_annotation_target_names_from_block(body: &[Stmt], names: &mut HashSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::AnnAssign { name, .. } => {
                names.insert(name.clone());
            }
            // Do not descend into nested function/class scopes.
            Stmt::Def { .. } | Stmt::Class { .. } => {}
            Stmt::If { branches, else_branch } => {
                for (_, branch) in branches {
                    collect_annotation_target_names_from_block(branch, names);
                }
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::While { body, else_branch, .. } => {
                collect_annotation_target_names_from_block(body, names);
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::For { body, else_branch, .. } => {
                collect_annotation_target_names_from_block(body, names);
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::Try { body, handlers, else_branch, finally_branch } => {
                collect_annotation_target_names_from_block(body, names);
                for handler in handlers {
                    collect_annotation_target_names_from_block(&handler.body, names);
                }
                if let Some(branch) = else_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
                if let Some(branch) = finally_branch {
                    collect_annotation_target_names_from_block(branch, names);
                }
            }
            Stmt::With { body, .. } => {
                collect_annotation_target_names_from_block(body, names);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_annotation_target_names_from_block(&arm.body, names);
                }
            }
            _ => {}
        }
    }
}

fn collect_declared_names(body: &[Stmt], pick: fn(&Stmt) -> Option<&Vec<String>>) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_declared_names_from_block(body, &mut names, pick);
    names
}

fn collect_declared_names_from_block(
    body: &[Stmt],
    names: &mut HashSet<String>,
    pick: fn(&Stmt) -> Option<&Vec<String>>,
) {
    for stmt in body {
        if let Some(declared) = pick(stmt) {
            names.extend(declared.iter().cloned());
            continue;
        }
        match stmt {
            Stmt::If { branches, else_branch } => {
                for (_, branch) in branches {
                    collect_declared_names_from_block(branch, names, pick);
                }
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::While { body, else_branch, .. } => {
                collect_declared_names_from_block(body, names, pick);
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::For { body, else_branch, .. } => {
                collect_declared_names_from_block(body, names, pick);
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::Try { body, handlers, else_branch, finally_branch } => {
                collect_declared_names_from_block(body, names, pick);
                for handler in handlers {
                    collect_declared_names_from_block(&handler.body, names, pick);
                }
                if let Some(branch) = else_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
                if let Some(branch) = finally_branch {
                    collect_declared_names_from_block(branch, names, pick);
                }
            }
            Stmt::With { body, .. } => {
                collect_declared_names_from_block(body, names, pick);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    collect_declared_names_from_block(&arm.body, names, pick);
                }
            }
            _ => {}
        }
    }
}

fn values_are_identical(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::None, ValueKind::None) => true,
        (ValueKind::Bool(x), ValueKind::Bool(y)) => x == y,
        (ValueKind::Int(x), ValueKind::Int(y)) => x == y,
        (ValueKind::PyInstance(x), ValueKind::PyInstance(y)) => Rc::ptr_eq(x, y),
        (ValueKind::PyClass(x), ValueKind::PyClass(y)) => Rc::ptr_eq(x, y),
        (ValueKind::UserFunction(x), ValueKind::UserFunction(y)) => Rc::ptr_eq(x, y),
        // BuiltinFunction values are singletons by name (static str identity).
        // This makes `type(5) is type(5)` True since both return the same name tag.
        (ValueKind::BuiltinFunction(x), ValueKind::BuiltinFunction(y)) => x == y,
        // Generators share an Rc<RefCell<...>> across clones (iter(g) returns
        // a clone of g).  Use Rc pointer equality so `g is iter(g)` is True,
        // matching CPython object identity semantics (#714).
        (ValueKind::Generator(x), ValueKind::Generator(y)) => Rc::ptr_eq(x, y),
        // For heap-backed values, identity is the shared backing-storage id
        // surfaced by `value_id()` — `b = a; a is b` is True after Rc-sharing
        // storage on clone (#305/#523).  Two distinct literals of the same
        // shape/value produce different ids, matching CPython.
        (ValueKind::BigInt(_), ValueKind::BigInt(_))
        | (ValueKind::List(_), ValueKind::List(_))
        | (ValueKind::Set(_), ValueKind::Set(_))
        | (ValueKind::Dict(_), ValueKind::Dict(_))
        | (ValueKind::Tuple(_), ValueKind::Tuple(_)) => match (a.value_id(), b.value_id()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        (ValueKind::Bytes(x), ValueKind::Bytes(y)) => Rc::ptr_eq(x, y),
        (ValueKind::PyModule(x), ValueKind::PyModule(y)) => Rc::ptr_eq(x, y),
        // BoundMethod / ClassBoundMethod / SuperProxy / SuperProxyClass: each
        // allocation gets a unique monotonic obj_id (stored in the Opaque
        // struct and surfaced by value_id()).  Two clones of the same value
        // share the same obj_id so `a is a` is True, while a second attribute
        // access produces a new obj_id so `obj.method is obj.method` is
        // False, matching CPython identity semantics (#722).
        (ValueKind::BoundMethod { .. }, ValueKind::BoundMethod { .. })
        | (ValueKind::ClassBoundMethod { .. }, ValueKind::ClassBoundMethod { .. })
        | (ValueKind::SuperProxy { .. }, ValueKind::SuperProxy { .. })
        | (ValueKind::SuperProxyClass { .. }, ValueKind::SuperProxyClass { .. }) => {
            match (a.value_id(), b.value_id()) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            }
        }
        // BuiltinObject covers built-in bound methods (list.append, dict.get,
        // etc.) and other host-created objects (frozenset, property, …).
        // Clones share the same Rc<RefCell<...>> state, so Rc pointer equality
        // is the correct identity test.  `a = lst.append; a is a` → True;
        // `lst.append is lst.append` → False (fresh BoundMethodState each
        // time) — matching CPython builtin_function_or_method identity (#722).
        (
            ValueKind::BuiltinObject { state: sa, .. },
            ValueKind::BuiltinObject { state: sb, .. },
        ) => Rc::ptr_eq(sa, sb),
        _ => false,
    }
}

/// Walk an expression tree and collect names bound by walrus operators (`:=`).
fn collect_walrus_targets_in_expr(
    expr: &Expr,
    names: &mut indexmap::IndexSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    match expr {
        Expr::Named { target, value } => {
            if !global_names.contains(target) && !nonlocal_names.contains(target) {
                names.insert(target.clone());
            }
            collect_walrus_targets_in_expr(value, names, global_names, nonlocal_names);
        }
        Expr::Binary { left, right, .. } => {
            collect_walrus_targets_in_expr(left, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(right, names, global_names, nonlocal_names);
        }
        Expr::Unary { expr: e, .. } => {
            collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
        }
        Expr::Compare { left, ops } => {
            collect_walrus_targets_in_expr(left, names, global_names, nonlocal_names);
            for (_, e) in ops {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::Call { func, args } => {
            collect_walrus_targets_in_expr(func, names, global_names, nonlocal_names);
            for a in args {
                collect_walrus_targets_in_expr(&a.value, names, global_names, nonlocal_names);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_walrus_targets_in_expr(cond, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(then, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(else_, names, global_names, nonlocal_names);
        }
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            for e in items {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::Starred(inner) => {
            collect_walrus_targets_in_expr(inner, names, global_names, nonlocal_names);
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    crate::ast::DictItem::Pair(k, v) => {
                        collect_walrus_targets_in_expr(k, names, global_names, nonlocal_names);
                        collect_walrus_targets_in_expr(v, names, global_names, nonlocal_names);
                    }
                    crate::ast::DictItem::DoubleSplat(e) => {
                        collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
                    }
                }
            }
        }
        Expr::Index { target, index } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(index, names, global_names, nonlocal_names);
        }
        Expr::Attr { target, .. } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
        }
        Expr::Slice { target, lower, upper, step } => {
            collect_walrus_targets_in_expr(target, names, global_names, nonlocal_names);
            for e in [lower, upper, step].iter().flat_map(|o| o.as_deref()) {
                collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
            }
        }
        Expr::FString(parts) => {
            // Walrus inside an f-string interpolation (or inside a nested
            // `{expr}` in the format spec) still binds in the enclosing
            // scope; recurse so the target name is recorded.
            use crate::ast::FStringPart;
            fn walk(
                parts: &[FStringPart],
                names: &mut indexmap::IndexSet<String>,
                global_names: &std::collections::HashSet<String>,
                nonlocal_names: &std::collections::HashSet<String>,
            ) {
                for part in parts {
                    if let FStringPart::Expr { expr, format_spec, .. } = part {
                        collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
                        if let Some(spec_parts) = format_spec {
                            walk(spec_parts, names, global_names, nonlocal_names);
                        }
                    }
                }
            }
            walk(parts, names, global_names, nonlocal_names);
        }
        _ => {}
    }
}

fn collect_assign_target_names(
    target: &AssignTarget,
    names: &mut indexmap::IndexSet<String>,
    global_names: &std::collections::HashSet<String>,
    nonlocal_names: &std::collections::HashSet<String>,
) {
    match target {
        AssignTarget::Name(n) => {
            if !global_names.contains(n) && !nonlocal_names.contains(n) {
                names.insert(n.clone());
            }
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_assign_target_names(t, names, global_names, nonlocal_names);
            }
        }
        AssignTarget::Attr(..) | AssignTarget::Index(..) => {}
        AssignTarget::Starred(inner) => {
            collect_assign_target_names(inner, names, global_names, nonlocal_names);
        }
    }
}

pub(crate) fn compute_def_bound_mask(
    params: &[crate::ast::FunctionParam],
    local_index: &HashMap<String, crate::bytecode::Reg>,
) -> u64 {
    let mut mask: u64 = 0;
    // Only parameters are guaranteed bound at function entry — they are set
    // by the call setup code before the body runs.  Body-level assignments
    // are NOT included here because a name can be read (as a local) before
    // it is assigned (e.g. `y = x; x = 9`), which would cause an unsound
    // unwrap.  The parameter-only subset is sufficient to eliminate the
    // None check for the most frequently read locals in hot inner loops.
    for param in params {
        if let Some(&idx) = local_index.get(&param.name)
            && idx < 64 {
                mask |= 1u64 << idx;
            }
    }
    mask
}

pub(crate) fn float_to_bigint(f: f64) -> Value {
    use crate::value::PyBigInt;
    // Convert via the decimal string representation of the f64's integer value.
    let s = format!("{:.0}", f);
    let n: PyBigInt = s.parse().unwrap_or_else(|_| PyBigInt::from(0i64));
    Value::bigint(n)
}

/// Coerce a `Value` to `f64` for numeric ops.  Returns the raw `f64` on
/// success or `None` for non-numeric types (including BigInt values that
/// overflow to ±inf) — the caller chooses the error wording so
/// CPython-parity messages stay precise at each call site.
///
/// For BigInt, use [`value_to_float`] instead when you need the
/// CPython-matching `OverflowError` (rather than a `TypeError`) for integers
/// too large to represent as a finite `f64`.
pub(crate) fn try_value_to_float(v: &Value) -> Option<f64> {
    match v.kind() {
        ValueKind::Float(f) => Some(f),
        ValueKind::Int(i) => Some(i as f64),
        ValueKind::Bool(b) => Some(if b { 1.0 } else { 0.0 }),
        ValueKind::BigInt(b) => {
            // `ToPrimitive::to_f64` always returns `Some` for BigInt (never
            // `None`); the result is `±inf` when the magnitude exceeds f64
            // range.  Filter those out so callers receive `None` rather than a
            // silent infinity that would bypass CPython's OverflowError.
            // Callers that need the proper OverflowError should use
            // `value_to_float` instead.
            b.to_f64().filter(|f| f.is_finite())
        }
        _ => None,
    }
}

/// Coerce a `Value` to `f64` with the `math`/builtins-style error message,
/// matching CPython's `math.sqrt`-family error wording.
///
/// Raises `OverflowError` (not `TypeError`) when a `BigInt` argument is too
/// large to represent as a finite `f64`, mirroring CPython's behaviour:
/// `math.sqrt(2**10000)` → `OverflowError: int too large to convert to float`.
pub(crate) fn value_to_float(v: &Value, ctx: &str) -> Result<f64> {
    // Handle BigInt before falling through to try_value_to_float so we can
    // distinguish a non-numeric type (TypeError) from an overflow (OverflowError).
    if let ValueKind::BigInt(b) = v.kind() {
        let f = b.to_f64().unwrap_or(f64::INFINITY);
        return if f.is_finite() {
            Ok(f)
        } else {
            Err(PyError::named(
                "OverflowError",
                "int too large to convert to float".to_string(),
            ))
        };
    }
    try_value_to_float(v).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("{ctx}: a float is required, not {}", v.repr()),
        )
    })
}

// `make_math_module()` / `make_sys_module()` removed — both are now
// generated by the `pyrust_module!` macro inside
// `crates/pyrust/src/builtin_modules/{math,sys}.rs`.  See
// `docs/builtin-migration.md` for the recipe.

/// Returns true if `expr` produces no observable side effects given the set of
/// locally-defined functions already confirmed pure (`pure_fns`).
///
/// A `Call` is pure only when the callee is a built-in declared `#[pure]` in
/// `pyrust_module! { … }` (reflected through `BuiltinReg::is_pure` — see
/// `crate::builtin_registry::is_pure`) or a locally-defined function in
/// `pure_fns`.  Indirect calls (methods, closures through computed
/// expressions) and calls to names registered as non-pure (or unknown) are
/// conservatively treated as impure.
///
/// Previously this used a hand-maintained `PURE_BUILTINS` list living
/// alongside this function.  The list drifted from the registry (issue #433),
/// so purity is now derived from the `#[pure]` attribute at each builtin's
/// declaration site.  Adding a `#[pure] fn foo(...)` to `pyrust_module!`
/// makes the optimizer DCE / fold `foo(…)` calls without any further edit.
fn is_pure_expr(
    expr: &Expr,
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    match expr {
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None => true,
        // A variable read is pure only when it refers to a local register in the
        // current function scope.  Reads of free variables (globals, names captured
        // from an enclosing scope) are inherently impure: the caller cannot
        // control whether the value changes between invocations, so memoising the
        // result via `CallMemo` would serve stale data.  Without this guard, a
        // function like `def f(): return counter` would be mis-classified as pure
        // and its first result permanently cached, hiding subsequent mutations of
        // `counter` (issue #346 correctness requirement).
        Expr::Var(n) => local_names.contains_key(n.as_str()),
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().all(|e| is_pure_expr(e, pure_fns, local_names))
        }
        Expr::Starred(inner) => is_pure_expr(inner, pure_fns, local_names),
        Expr::Dict(items) => items.iter().all(|item| match item {
            crate::ast::DictItem::Pair(k, v) => {
                is_pure_expr(k, pure_fns, local_names) && is_pure_expr(v, pure_fns, local_names)
            }
            crate::ast::DictItem::DoubleSplat(e) => is_pure_expr(e, pure_fns, local_names),
        }),
        Expr::Unary { expr, .. } => is_pure_expr(expr, pure_fns, local_names),
        Expr::Binary { left, right, .. } => {
            is_pure_expr(left, pure_fns, local_names) && is_pure_expr(right, pure_fns, local_names)
        }
        Expr::Compare { left, ops } => {
            is_pure_expr(left, pure_fns, local_names)
                && ops.iter().all(|(_, e)| is_pure_expr(e, pure_fns, local_names))
        }
        Expr::Ternary { cond, then, else_ } => {
            is_pure_expr(cond, pure_fns, local_names)
                && is_pure_expr(then, pure_fns, local_names)
                && is_pure_expr(else_, pure_fns, local_names)
        }
        // A lambda expression allocates a fresh Rc<UserFunction> on every evaluation,
        // so any enclosing function that returns one is not pure in the identity sense.
        Expr::Lambda { .. } => false,
        Expr::Call { func, args } => {
            // Only direct calls to named callees can be pure.  Two shapes
            // qualify:
            //   1. `name(…)`         — `Expr::Var(name)` callee.
            //   2. `module.name(…)`  — `Expr::Attr { Var(module), name }`
            //      callee, e.g. `math.sqrt(x)`.  The registry keys
            //      module-namespaced builtins by `module.name` (see
            //      `builtin_modules/mod.rs::all_regs`), so we look them up
            //      under that joined form.  Anything more indirect
            //      (`a.b.c(…)`, computed callees, method calls on values)
            //      stays conservatively impure.
            let callee_is_pure = match func.as_ref() {
                Expr::Var(name) => {
                    // Local fns already confirmed pure (`pure_fns`) take
                    // precedence so user-defined names shadowing a builtin
                    // don't accidentally hit the registry — a stray builtin
                    // name shadowed by a local can't be mis-classified.
                    // `is_pure` returns `false` both for "registered but
                    // impure" (`print`, `open`, …) and "not registered", so
                    // the conservative default collapses both paths into a
                    // single check.
                    pure_fns.contains(name.as_str())
                        || crate::builtin_registry::is_pure(name)
                }
                Expr::Attr { target, name } => {
                    // Module-attribute call.  Only treat as pure when the
                    // target is a bare module-name `Var` (so `math.sqrt`
                    // qualifies but `obj.method` / `a.b.c` do not) AND the
                    // joined `module.name` is registered `#[pure]`.  Method
                    // calls on user instances are always impure because we
                    // can't see through the receiver here.
                    if let Expr::Var(module) = target.as_ref() {
                        // Local shadowing of the module name disables the
                        // registry lookup, mirroring the bare-`Var` arm.
                        if pure_fns.contains(module.as_str()) {
                            false
                        } else {
                            let joined = format!("{module}.{name}");
                            crate::builtin_registry::is_pure(&joined)
                        }
                    } else {
                        false
                    }
                }
                _ => {
                    // Indirect call (computed callee, deeper attr chain) —
                    // conservatively impure.
                    false
                }
            };
            if !callee_is_pure {
                return false;
            }
            args.iter().all(|a| is_pure_expr(&a.value, pure_fns, local_names))
        }
        Expr::Attr { target, .. } => is_pure_expr(target, pure_fns, local_names),
        Expr::Index { target, index } => {
            is_pure_expr(target, pure_fns, local_names) && is_pure_expr(index, pure_fns, local_names)
        }
        Expr::Slice { target, lower, upper, step } => {
            is_pure_expr(target, pure_fns, local_names)
                && lower.as_deref().is_none_or(|e| is_pure_expr(e, pure_fns, local_names))
                && upper.as_deref().is_none_or(|e| is_pure_expr(e, pure_fns, local_names))
                && step.as_deref().is_none_or(|e| is_pure_expr(e, pure_fns, local_names))
        }
        // Comprehensions involve iteration (GetIter, ForIter) which may call
        // __iter__/__next__ — conservatively treat as impure.
        Expr::ListComp { .. } | Expr::DictComp { .. } | Expr::SetComp { .. } | Expr::GenExp { .. } => false,
        // Walrus has a side effect (assignment).
        Expr::Named { .. } => false,
        Expr::FString(parts) => {
            use crate::ast::FStringPart;
            fn check_parts(
                parts: &[FStringPart],
                pure_fns: &std::collections::HashSet<String>,
                local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
            ) -> bool {
                parts.iter().all(|p| match p {
                    FStringPart::Literal(_) => true,
                    FStringPart::Expr { expr, format_spec, .. } => {
                        is_pure_expr(expr, pure_fns, local_names)
                            && format_spec
                                .as_ref()
                                .is_none_or(|sp| check_parts(sp, pure_fns, local_names))
                    }
                })
            }
            check_parts(parts, pure_fns, local_names)
        }
        // yield/yield from always have side effects (generator suspension).
        Expr::Yield(_) | Expr::YieldFrom(_) => false,
    }
}

/// Returns true if every statement in `body` is free of observable side effects.
///
/// `pure_fns` is the set of locally-defined functions already confirmed pure;
/// calls to names outside this set and outside the registry's `#[pure]`-marked
/// builtins (see `crate::builtin_registry::is_pure`) are treated as impure.
/// Attribute/index mutation, global/nonlocal declarations, imports, and
/// `with` blocks are always impure.
///
/// `local_names` is the local-variable index for the function being analysed.
/// Variable reads (`Expr::Var`) are pure only when the name is a function-local;
/// reads of free variables (module globals, outer-scope names) are impure
/// because the memoisation in `CallMemo` would cache the first result and
/// silently hide subsequent mutations.
pub(crate) fn is_pure_body(
    body: &[Stmt],
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    body.iter().all(|s| is_pure_stmt(s, pure_fns, local_names))
}

fn is_pure_stmt(
    stmt: &Stmt,
    pure_fns: &std::collections::HashSet<String>,
    local_names: &std::collections::HashMap<String, crate::bytecode::Reg>,
) -> bool {
    match stmt {
        // Explicit side effects on outer state.
        Stmt::Global(_) | Stmt::Nonlocal(_) => false,
        // Object / container mutation.
        Stmt::AttrAssign { .. } | Stmt::IndexAssign { .. } | Stmt::SliceAssign { .. } => false,
        // Deletion and imports can affect shared state.
        Stmt::Delete(_) | Stmt::Import { .. } | Stmt::ImportFrom { .. } => false,
        // `with` typically wraps I/O or resource-management side effects.
        Stmt::With { .. } => false,

        // Assignments and augmented assignments are local writes → pure if RHS is.
        Stmt::Assign(_, expr) | Stmt::Expr(expr) => is_pure_expr(expr, pure_fns, local_names),
        Stmt::AugAssign { expr, .. } => is_pure_expr(expr, pure_fns, local_names),
        Stmt::Return(Some(expr)) => is_pure_expr(expr, pure_fns, local_names),
        Stmt::Return(None) => true,
        Stmt::Assert { test, msg } => {
            is_pure_expr(test, pure_fns, local_names)
                && msg.as_ref().is_none_or(|e| is_pure_expr(e, pure_fns, local_names))
        }
        Stmt::Raise { expr, cause } => {
            expr.as_ref().is_none_or(|e| is_pure_expr(e, pure_fns, local_names))
                && cause.as_ref().is_none_or(|e| is_pure_expr(e, pure_fns, local_names))
        }

        // Control flow: recurse into sub-blocks.
        Stmt::If { branches, else_branch } => {
            branches
                .iter()
                .all(|(cond, blk)| is_pure_expr(cond, pure_fns, local_names) && is_pure_body(blk, pure_fns, local_names))
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }
        Stmt::While { cond, body, else_branch } => {
            is_pure_expr(cond, pure_fns, local_names)
                && is_pure_body(body, pure_fns, local_names)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }
        Stmt::For { iter, body, else_branch, .. } => {
            is_pure_expr(iter, pure_fns, local_names)
                && is_pure_body(body, pure_fns, local_names)
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }
        Stmt::Try { body, handlers, else_branch, finally_branch } => {
            is_pure_body(body, pure_fns, local_names)
                && handlers.iter().all(|h| is_pure_body(&h.body, pure_fns, local_names))
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
                && finally_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }

        // Annotated assignment modifies __annotations__ dict — impure at module/class scope.
        Stmt::AnnAssign { .. } => false,
        // Nested definitions always allocate a fresh heap object (Rc<UserFunction> /
        // PyClass), so any function that defines and returns one is non-pure: successive
        // calls with identical arguments produce values with distinct identities.
        Stmt::Def { .. } | Stmt::Class { .. } => false,
        Stmt::Pass | Stmt::Break | Stmt::Continue => true,
        Stmt::Match { subject, arms } => {
            is_pure_expr(subject, pure_fns, local_names)
                && arms
                    .iter()
                    .all(|arm| is_pure_body(&arm.body, pure_fns, local_names))
        }
    }
}

/// Round a float to the nearest integer using banker's rounding (round half to even),
/// matching CPython's `round(x)` with no ndigits argument.
pub(crate) fn py_round_half_even(v: f64) -> i64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff < 0.5 {
        floor as i64
    } else if diff > 0.5 {
        (floor + 1.0) as i64
    } else {
        // Exactly 0.5: round to even
        let floor_i = floor as i64;
        if floor_i % 2 == 0 {
            floor_i
        } else {
            floor_i + 1
        }
    }
}

/// Round a float to nearest using banker's rounding, returning f64.
/// Used by round(x, n) for float inputs.
pub(crate) fn py_round_half_even_f64(v: f64) -> f64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else {
        // Exactly 0.5: round to even
        let floor_i = floor as i64;
        if floor_i % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    }
}

/// Modular exponentiation: (base^exp) % modulus for i64.
pub(crate) fn modpow_i64(base: i64, exp: u64, modulus: i64) -> i64 {
    if modulus == 1 {
        return 0;
    }
    let mut result: i64 = 1;
    let mut base = ((base % modulus) + modulus) % modulus;
    let mut exp = exp;
    while exp > 0 {
        if exp % 2 == 1 {
            result = (result * base) % modulus;
        }
        exp >>= 1;
        base = (base * base) % modulus;
    }
    result
}

/// CPython's `_Py_HashDouble` algorithm for float hashing.
///
/// Implements the Mersenne-prime hash (P = 2^61 - 1) that CPython uses for
/// floating-point values.  The float is represented as `m * 2^e` (with integer
/// `m` and `e`) and the hash is `m * 2^e mod P`, signed to match the float's
/// sign.  The `-1 → -2` sentinel remap is applied at the end.
///
/// Special cases:
/// - `+inf` → `314159`, `-inf` → `-314159`  (CPython: `sys.hash_info.inf`)
/// - `NaN`  → `0`  (CPython uses object-identity hash for NaN; pyrust returns
///   `sys.hash_info.nan = 0` as a stable fallback since float NaN values are
///   not objects with stable addresses in this VM)
/// - `0.0` / `-0.0` → `0`
/// - Integral floats (e.g. `1.0`, `2.0`) hash the same as the corresponding
///   integer: `hash(1.0) == hash(1)` (CPython invariant).
pub(crate) fn py_hash_float(v: f64) -> i64 {
    // CPython Mersenne prime: P = 2^61 - 1.
    const P: u64 = (1u64 << 61) - 1;

    if v.is_infinite() {
        return if v > 0.0 { 314159 } else { -314159 };
    }
    if v.is_nan() {
        // CPython calls PyObject_GenericHash (id-based) for NaN; pyrust
        // doesn't have stable float object identity, so return the canonical
        // sys.hash_info.nan value (0) as a stable substitute.
        return 0;
    }
    if v == 0.0 {
        return 0;
    }

    // Decompose v using IEEE 754 bits: [sign(1)][exponent(11)][mantissa(52)].
    let bits = v.to_bits();
    let sign: i64 = if v < 0.0 { -1 } else { 1 };
    let biased_exp = ((bits >> 52) & 0x7ff) as i64;
    let mantissa_bits = bits & 0x000f_ffff_ffff_ffff;

    // Build (m, e) such that |v| = m * 2^e with m a positive integer.
    // For normal numbers:  m = mantissa | (1 << 52),  e = biased_exp - 1023 - 52
    // For subnormal numbers: m = mantissa,             e = 1 - 1023 - 52 = -1074
    let (m, e): (u64, i64) = if biased_exp == 0 {
        (mantissa_bits, -1074)
    } else {
        (mantissa_bits | (1u64 << 52), biased_exp - 1023 - 52)
    };

    // Compute h = m * 2^e mod P.
    //
    // Key identity: 2^61 ≡ 1 (mod P), so only the residue of e mod 61 matters
    // for the shift direction.
    //
    // Positive e: h = m * 2^(e mod 61) mod P.
    //   m fits in 53 bits, 2^(e mod 61) fits in 61 bits; product ≤ 2^114 → u128.
    //
    // Negative e: h = m * inv(2^|e|) mod P.
    //   inv(2^k) ≡ 2^(61 - k mod 61) mod P  when k mod 61 ≠ 0,
    //            ≡ 1                          when k mod 61 == 0.
    //   (Proof: 2^(k mod 61) * 2^(61 - k mod 61) = 2^61 ≡ 1 mod P.)
    let h: u64 = if e >= 0 {
        let shift = (e as u64) % 61;
        ((m as u128 * (1u128 << shift)) % (P as u128)) as u64
    } else {
        let neg_e_mod = ((-e) as u64) % 61;
        let inv_shift = if neg_e_mod == 0 { 0u64 } else { 61 - neg_e_mod };
        ((m as u128 * (1u128 << inv_shift)) % (P as u128)) as u64
    };

    // Apply sign, then remap the C-level sentinel -1 to -2.
    let signed = h as i64 * sign;
    if signed == -1 { -2 } else { signed }
}

/// Resolve zero-argument `super()` as CPython 3.12 does.
///
/// CPython synthesises a `__class__` cell variable for every function compiled
/// directly inside a class body and implicitly passes `__class__` and the first
/// positional argument (typically `self` or `cls`) to zero-arg `super()`.
///
/// pyrust mirrors this by:
/// - Writing `__class__` into the class env after the class body runs
///   (`Insn::MakeClass` in vm.rs).  Methods capture the same env at
///   `MakeFunction` time, so `__class__` is always reachable through the
///   method's env chain.
/// - `FnCode::is_class_method` — set to `true` only for functions compiled
///   directly inside a class body (`compile_def` when `self.is_class_body`).
///   Nested functions inside methods get `false`.
///
/// Returns `(class_value, self_or_cls_value)` on success.
///
/// Returns `Err(RuntimeError("super(): no arguments"))` when:
/// - Called at module/script scope (no Function frame on the stack).
/// - Called from a plain function or a nested function inside a method
///   (innermost Function frame has `is_class_method == false`).
/// - `__class__` is not reachable in the env chain (should not happen for
///   valid class methods — defensive guard).
/// - Register 0 (first positional arg) is unset.
pub(crate) fn resolve_zero_arg_super(
    interp: &Interpreter,
) -> crate::error::Result<(Value, Value)> {
    // The INNERMOST Function frame must itself be a direct class method.
    // CPython's zero-arg super() uses a magic `__class__` cell that is only
    // synthesised for functions compiled directly inside a class body; nested
    // functions (`def inner(): super()` inside a method) do not get that cell
    // and therefore zero-arg super() must raise RuntimeError.
    //
    // We find the innermost Function frame (the one currently executing) and
    // check `is_class_method`.  If it is false we immediately fail — even if
    // some outer frame IS a class method.
    let innermost_fn_frame = interp
        .vm_frame_views
        .iter()
        .rev()
        .find(|v| v.kind == FrameKind::Function);

    let Some(view) = innermost_fn_frame else {
        // Called at module/script scope — no function frame at all.
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    };

    if !view.is_class_method {
        // Innermost function is not a direct class method (e.g. nested inner
        // function, standalone function).
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    }

    // Verify that __class__ is reachable through the env chain.  It is written
    // by MakeClass into the class env after the class body finishes; the method
    // captured the same Rc<RefCell<Environment>> at MakeFunction time.
    let class_val = lookup_name_in_env(&interp.env, "__class__")?;
    let Some(class_val) = class_val else {
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    };

    // Read register 0 — the first positional parameter (`self` or `cls`).
    if view.regs_len == 0 {
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    }
    // SAFETY: view.regs_ptr is a NonNull<Value> pointing at the frame's
    // register file; the frame is suspended inside call_user_function_expanded
    // (which pushed the VmFrameView) and has not been freed.  The single-
    // element read does not alias any &mut [Value] (PR #646 removed those).
    let first_arg = unsafe { view.regs_ptr.as_ref() };
    if first_arg.is_unset() {
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    }

    Ok((class_val, first_arg.clone()))
}

#[cfg(test)]
mod purity_tests {
    use super::is_pure_body;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use std::collections::{HashMap, HashSet};

    fn parse_body(src: &str) -> Vec<crate::ast::Stmt> {
        let tokens = Lexer::new(src).expect("lex failed").into_tokens();
        Parser::new(tokens).parse_program().expect("parse failed")
    }

    /// Module-namespaced builtins (`math.sqrt`, `math.sin`, …) must be
    /// recognised as pure by the optimizer's call gate.  This is the
    /// headline acceptance criterion from #433 — the legacy hardcoded
    /// `PURE_BUILTINS` list couldn't express prefixed names, and the
    /// initial registry-derived rewrite only looked at bare `Expr::Var`
    /// callees so `math.sqrt(x)` slipped through to the "indirect call"
    /// branch and stayed conservatively impure.
    #[test]
    fn module_namespaced_math_calls_are_pure() {
        let body = parse_body("y = math.sqrt(x)\nz = math.sin(y)\nreturn z\n");
        // Treat x, y, z as local registers so only the callee purity is tested,
        // not whether variable reads are free variables.
        let locals: HashMap<String, u32> = [("x", 0u32), ("y", 1), ("z", 2)]
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        assert!(
            is_pure_body(&body, &HashSet::new(), &locals),
            "math.sqrt / math.sin must register as pure (registry-keyed lookup)"
        );
    }

    /// Method calls on values must remain impure — the receiver can be
    /// any user instance whose method has side effects, and we don't
    /// know the receiver's type at AST-purity time.
    #[test]
    fn value_method_calls_stay_impure() {
        let body = parse_body("y = obj.frobnicate(x)\nreturn y\n");
        assert!(
            !is_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "obj.method(...) calls must be conservatively impure"
        );
    }

    /// Deeper attribute chains (`a.b.c(…)`) don't qualify — only
    /// `module.name(…)` is registry-checkable.
    #[test]
    fn nested_attribute_calls_stay_impure() {
        let body = parse_body("y = a.b.c(x)\nreturn y\n");
        assert!(
            !is_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "a.b.c(...) calls must be conservatively impure"
        );
    }

    /// Builtin print(...) is registered impure; the registry lookup
    /// must propagate that to the body gate.  This is the mirror of
    /// `module_namespaced_math_calls_are_pure` for the impure side.
    #[test]
    fn impure_builtins_are_rejected() {
        let body = parse_body("print(x)\nreturn x\n");
        assert!(
            !is_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "print(...) is registered impure and must NOT pass the gate"
        );
    }

    /// A body containing a nested `def` must be classified impure.
    /// Each execution of `def f(): ...` allocates a fresh Rc<UserFunction>
    /// with a unique identity; memoising the outer call would skip that
    /// allocation and return the same function object every time, breaking
    /// `f1 is f2 == False` (#769).
    #[test]
    fn nested_def_is_impure() {
        let body = parse_body("def f(): pass\nreturn f\n");
        assert!(
            !is_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "body with nested def must be impure (fixes #769)"
        );
    }

    /// A body containing a nested `class` must be classified impure for the
    /// same reason as `nested_def_is_impure`: each class statement allocates
    /// a fresh PyClass object whose identity is observable via `is` / `id()`.
    #[test]
    fn nested_class_is_impure() {
        let body = parse_body("class C: pass\nreturn C\n");
        assert!(
            !is_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "body with nested class must be impure (fixes #769)"
        );
    }

    /// A `lambda` expression allocates a fresh Rc<UserFunction> on every
    /// evaluation, so a body that contains a lambda return is not pure in
    /// the identity sense.
    #[test]
    fn lambda_expr_is_impure() {
        let body = parse_body("return lambda x: x + 1\n");
        assert!(
            !is_pure_body(&body, &HashSet::new(), &HashMap::new()),
            "body returning a lambda must be impure (fixes #769)"
        );
    }
}


