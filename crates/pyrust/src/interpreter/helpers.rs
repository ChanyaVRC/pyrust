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
        (ValueKind::Bytes(x), ValueKind::Bytes(y)) => Ok(x.as_slice().cmp(y.as_slice())),
        // bytearray <=> bytearray comparison.
        (ValueKind::BuiltinObject { ops: aops, .. }, ValueKind::BuiltinObject { ops: bops, .. })
            if aops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
               && bops.type_name() == pyrust_builtins::bytearray::TYPE_NAME =>
        {
            let a_rc = pyrust_builtins::bytearray::as_bytearray_rc(a)
                .expect("bytearray rc");
            let b_rc = pyrust_builtins::bytearray::as_bytearray_rc(b)
                .expect("bytearray rc");
            Ok(a_rc.borrow().as_slice().cmp(b_rc.borrow().as_slice()))
        }
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
        // Two slice objects compare as their `(start, stop, step)` tuples
        // (issue #2127), matching CPython's `slice_richcompare`.  Mixed
        // None/int bounds raise exactly the TypeError the equivalent tuple
        // comparison would (e.g. `slice(None,2) < slice(1,2)` → `None < 1`).
        (
            ValueKind::BuiltinObject { ops: aops, .. },
            ValueKind::BuiltinObject { ops: bops, .. },
        ) if aops.type_name() == pyrust_builtins::slice::TYPE_NAME
            && bops.type_name() == pyrust_builtins::slice::TYPE_NAME =>
        {
            let (a_start, a_stop, a_step) =
                pyrust_builtins::slice::slice_fields(a).expect("slice fields");
            let (b_start, b_stop, b_step) =
                pyrust_builtins::slice::slice_fields(b).expect("slice fields");
            for (x, y) in [(&a_start, &b_start), (&a_stop, &b_stop), (&a_step, &b_step)] {
                // Tuple ordering scans the equal prefix with `==` (so equal
                // unorderable fields like two `None`s don't error) and only
                // applies the ordering op to the first *differing* field.
                if x == y {
                    continue;
                }
                return compare_values_with_op(x, y, op_name);
            }
            Ok(std::cmp::Ordering::Equal)
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
    // Borrow the class to read its own attrs and recurse into the base chain by
    // reference, cloning out only the matched `Value`.  Distinct classes are
    // distinct `RefCell`s, so recursing under the current borrow never
    // conflicts (same pattern as `class_chain_contains_name`, #1967).  Avoiding
    // the previous per-node `base`/`extra_bases` `Rc`+`Vec` clones removes the
    // dominant allocation churn on the exception-construction path, where this
    // is called twice per `raise` (for `__new__` and `__init__`).
    let borrowed = class.borrow();
    if let Some(v) = borrowed.attrs.get(name) {
        return Some(v.clone());
    }
    let has_explicit_base = borrowed.base.is_some();
    // Issue #2075: when a class participates in *multiple* inheritance, plain
    // depth-first recursion ("primary base's full ancestry, then the extra
    // bases") is NOT C3: in a diamond `D(B, C)` it descends `D → B → A` and
    // returns `A`'s attribute before ever considering the sibling `C` that
    // overrides it.  CPython resolves attributes by scanning the C3 `__mro__`
    // left-to-right and returning the first class whose *own* dict defines the
    // name.  Switch to that order here whenever this class has extra bases.
    //
    // The fast single-inheritance path below (no `extra_bases`) is left exactly
    // as before: for a linear chain depth-first recursion already equals C3, so
    // ordinary classes and the hot exception-construction path pay nothing.
    if !borrowed.extra_bases.is_empty() {
        // Drop the borrow before computing the MRO (which borrows each class).
        drop(borrowed);
        for cls in c3_linearize_classes(class) {
            if let Some(v) = cls.borrow().attrs.get(name) {
                return Some(v.clone());
            }
        }
        return None;
    }
    if let Some(base) = &borrowed.base {
        if let Some(v) = lookup_class_attr(base, name) {
            return Some(v);
        }
    }
    // Issue #1378: every class implicitly has `object` as its ultimate ancestor
    // (CPython's invariant).  When the MRO chain terminates (no explicit primary
    // base) and the class is not itself `object`, fall through to the object
    // singleton's attrs.  This mirrors class_is_subclass_of (which returns true
    // for any class when expected is object) and class_mro_items (which appends
    // object at the end of every MRO).
    //
    // Without this fallback, built-in exception classes whose base chain ends at
    // BaseException (base==None) never reached object's attrs — so
    // `hasattr(Exception, '__init_subclass__')` was False.
    //
    // Issue #1537: primitive class singletons (int, str, list, …) now set their
    // `base` to the `object` singleton explicitly, so `has_explicit_base` is
    // true for them and this fallback branch is skipped for them.  The
    // `is_primitive_class` guard is retained as a safety net for any class that
    // might lack an explicit base but still should not fall through to object.
    if !has_explicit_base && !is_primitive_class(class) {
        let obj = object_class_singleton();
        if !Rc::ptr_eq(class, &obj) {
            return lookup_class_attr(&obj, name);
        }
    }
    None
}

/// C3 linearization of `class`, returning the MRO as a `Vec` of class pointers
/// (the same order as `__mro__` / `class_mro_items`), with the `object`
/// singleton appended last.  Unlike `class_mro_items`, this returns class
/// pointers (no `Value` wrapping) and is infallible: it is only ever called on
/// classes that were successfully created (so a consistent linearization was
/// already verified at class-creation time).  Used by `lookup_class_attr` to
/// scan multiple-inheritance bases in C3 order (issue #2075).
///
/// Runs the identical C3 algorithm as `class_mro_items` (which `__mro__` and
/// `mro()` use), so the two always agree on order.  They are kept separate
/// because `class_mro_items` is fallible — it reports a `TypeError` for an
/// inconsistent linearization at class-creation time — whereas this variant is
/// only reached after a class already exists and so never needs to fail.
pub(crate) fn c3_linearize_classes(
    class: &Rc<RefCell<PyClass>>,
) -> Vec<Rc<RefCell<PyClass>>> {
    fn linearize(c: &Rc<RefCell<PyClass>>) -> Vec<Rc<RefCell<PyClass>>> {
        let (base, extra_bases) = {
            let b = c.borrow();
            (b.base.clone(), b.extra_bases.clone())
        };
        let mut all_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
        if let Some(b) = base {
            all_bases.push(b);
        }
        all_bases.extend(extra_bases);
        if all_bases.is_empty() {
            return vec![Rc::clone(c)];
        }
        let mut lists: Vec<Vec<Rc<RefCell<PyClass>>>> =
            all_bases.iter().map(linearize).collect();
        lists.push(all_bases);

        let mut result = vec![Rc::clone(c)];
        loop {
            lists.retain(|l| !l.is_empty());
            if lists.is_empty() {
                break;
            }
            let mut chosen: Option<Rc<RefCell<PyClass>>> = None;
            'outer: for list in &lists {
                let head_ptr = Rc::as_ptr(&list[0]);
                for other in &lists {
                    for tail in other.iter().skip(1) {
                        if Rc::as_ptr(tail) == head_ptr {
                            continue 'outer;
                        }
                    }
                }
                chosen = Some(Rc::clone(&list[0]));
                break;
            }
            // No consistent head: fall back to the first remaining head so the
            // scan still terminates.  This cannot happen for a validly-created
            // class (the MRO was checked at creation), but we never panic.
            let chosen = chosen.unwrap_or_else(|| Rc::clone(&lists[0][0]));
            let chosen_ptr = Rc::as_ptr(&chosen);
            result.push(chosen);
            for list in &mut lists {
                if !list.is_empty() && Rc::as_ptr(&list[0]) == chosen_ptr {
                    list.remove(0);
                }
            }
        }
        result
    }

    let mut mro = linearize(class);
    let obj = object_class_singleton();
    if !mro.iter().any(|c| Rc::ptr_eq(c, &obj)) {
        mro.push(obj);
    }
    mro
}

thread_local! {
    static OBJECT_CLASS: Rc<RefCell<PyClass>> = {
        // Issue #1047: object.__init_subclass__ is a no-op classmethod in
        // CPython.  Register the builtin sentinel so that
        // `super().__init_subclass__(**kwargs)` inside user __init_subclass__
        // methods finds it when the MRO walk reaches `object`.
        //
        // Issue #1256: also register the common object dunders so that
        // `hasattr(object, '__str__')` returns True and `super().__str__()`
        // in user classes resolves via MRO to the registered handler.
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        for dunder in &[
            "__init_subclass__",
            "__subclasshook__",
            "__getattribute__",
            "__setattr__",
            "__delattr__",
            "__str__",
            "__repr__",
            "__eq__",
            "__ne__",
            "__hash__",
            "__init__",
            "__new__",
            "__lt__",
            "__le__",
            "__gt__",
            "__ge__",
            "__format__",
            // Issue #2151: object-protocol methods every object inherits.
            // `obj.__sizeof__()` / `obj.__dir__()` / `obj.__reduce__()` /
            // `obj.__reduce_ex__(p)` resolve here for all values whose class
            // chains to `object`.
            "__sizeof__",
            "__dir__",
            "__reduce__",
            "__reduce_ex__",
        ] {
            let qualified: &'static str =
                Box::leak(format!("object.{dunder}").into_boxed_str());
            attrs.insert((*dunder).to_string(), Value::builtin_function(qualified));
        }
        Rc::new(RefCell::new(PyClass::new("object", "object", None, attrs)))
    };

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

    /// Per-thread metaclass singleton for `type`.  In CPython, `type` is
    /// both a callable and a class — `type(int)` returns `<class 'type'>`,
    /// and `isinstance(int, type)` is True.  Mirrors the `OBJECT_CLASS`
    /// pattern (issue #1312).
    ///
    /// Issue #1537: `type.__bases__ == (object,)` in CPython.  Setting the
    /// explicit base lets `lookup_class_attr` walk to `object` so that
    /// `hasattr(type, '__init_subclass__')` returns True.
    static TYPE_CLASS: Rc<RefCell<PyClass>> = {
        let obj = OBJECT_CLASS.with(Rc::clone);
        // Issue #1385: register type.__new__ and type.__init__ so that
        // `super().__new__(mcs, name, bases, namespace)` and
        // `super().__init__(name, bases, namespace)` inside custom metaclass
        // methods resolve to these builtins instead of falling through to
        // object.__new__ / object.__init__ which reject the extra arguments.
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__new__".to_string(),
            Value::builtin_function("type.__new__"),
        );
        attrs.insert(
            "__init__".to_string(),
            Value::builtin_function("type.__init__"),
        );
        // Issue #1956: register `type.__call__` so that `super().__call__(*a)`
        // inside a metaclass `__call__` override resolves (via the metaclass
        // MRO super-walk) to the default construct.
        attrs.insert(
            "__call__".to_string(),
            Value::builtin_function("type.__call__"),
        );
        // Issue #2128: register the default `type.__prepare__` so
        // `hasattr(type, '__prepare__')` is true, `type.__prepare__(name, bases)`
        // returns a fresh dict, and `super().__prepare__(...)` resolves inside a
        // custom metaclass.  It is a classmethod (receives the metaclass).
        attrs.insert(
            "__prepare__".to_string(),
            Value::builtin_function("type.__prepare__"),
        );
        let cls = Rc::new(RefCell::new(PyClass::new(
            "type",
            "type",
            Some(Rc::clone(&obj)),
            attrs,
        )));
        obj.borrow().subclasses.borrow_mut().push(Rc::downgrade(&cls));
        cls
    };

    /// Per-thread `PyClass` singleton for the `method` type.  In CPython,
    /// `type(instance.method)` returns `<class 'method'>` — a proper class
    /// whose metatype is `type`, so `type(type(c.m)) is type` holds.
    /// Issue #1528: previously `type(c.m)` returned a `BuiltinFunction("method")`
    /// sentinel, so `type(type(c.m))` resolved to `builtin_function_or_method`.
    static METHOD_TYPE: Rc<RefCell<PyClass>> = Rc::new(RefCell::new(PyClass::new(
        "method",
        "method",
        None,
        IndexMap::new(),
    )));

    /// Per-thread `PyClass` singleton for the `function` type.  In CPython,
    /// `type(lambda: None)` returns `<class 'function'>` — a proper class
    /// whose metatype is `type`, so `type(type(lambda: None)) is type` holds.
    /// Issue #1528: previously `type(f)` for a user-defined function returned
    /// a `BuiltinFunction("function")` sentinel.
    static FUNCTION_TYPE: Rc<RefCell<PyClass>> = Rc::new(RefCell::new(PyClass::new(
        "function",
        "function",
        None,
        IndexMap::new(),
    )));

    /// Per-thread `PyClass` singleton for the `range` type.  In CPython,
    /// `range` is a proper class (`type(range(5)) is range`), not a builtin
    /// function.  This singleton lets `type(range(5))` return a real `PyClass`
    /// and enables `issubclass(range, Sequence)` via `extra_bases` registration
    /// in `register_abc_extra_bases`.  Issues #1793, #1800.
    static RANGE_CLASS: Rc<RefCell<PyClass>> = {
        let obj = OBJECT_CLASS.with(Rc::clone);
        let cls = Rc::new(RefCell::new(PyClass::new(
            "range",
            "range",
            Some(Rc::clone(&obj)),
            IndexMap::new(),
        )));
        obj.borrow().subclasses.borrow_mut().push(Rc::downgrade(&cls));
        cls
    };

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
        let cell = std::cell::RefCell::new(std::collections::HashMap::with_capacity(15));
        PRIMITIVE_CLASSES.with(|c| {
            let mut m = cell.borrow_mut();
            for (class, name) in [
                (&c.bool_class, "bool"),
                (&c.bytearray_class, "bytearray"),
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
            // Issue #1451: NoneType, NotImplementedType, and ellipsis were
            // added to PrimitiveClasses by #1403 but not registered here,
            // causing calls like `type(None)()` to fall through to
            // `call_class_expanded` which allocated a bogus PyInstance.
            // CPython 3.12: zero-arg call returns the singleton; any
            // arguments raise TypeError "<TypeName> takes no arguments".
            m.insert(
                Rc::as_ptr(&c.none_class),
                none_ctor as crate::builtin_registry::BuiltinDispatchFn,
            );
            m.insert(
                Rc::as_ptr(&c.notimplemented_class),
                notimplemented_ctor as crate::builtin_registry::BuiltinDispatchFn,
            );
            m.insert(
                Rc::as_ptr(&c.ellipsis_class),
                ellipsis_ctor as crate::builtin_registry::BuiltinDispatchFn,
            );
        });
        // `type` metaclass: every `PyClass` value is an instance of `type`
        // in CPython.  Register the TYPE_CLASS singleton here so that
        // calling `type(x)` dispatches to the existing "type" registry entry
        // without going through `call_class_expanded` (issue #1312).
        TYPE_CLASS.with(|t| {
            if let Some(dispatch) = crate::builtin_registry::lookup("type") {
                cell.borrow_mut().insert(Rc::as_ptr(t), dispatch);
            }
        });
        // Issues #1793, #1800: `range` is a proper class in CPython, so
        // `range(1, 10)` is a constructor call on the range class.  Register
        // it so `call_function_expanded`'s PyClass arm dispatches to the
        // existing "range" registry fn instead of falling through to
        // `call_class_expanded` which would allocate a bogus PyInstance.
        RANGE_CLASS.with(|r| {
            if let Some(dispatch) = crate::builtin_registry::lookup("range") {
                cell.borrow_mut().insert(Rc::as_ptr(r), dispatch);
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
    pub(crate) bytearray_class: Rc<RefCell<PyClass>>,
    pub(crate) bytes_class: Rc<RefCell<PyClass>>,
    pub(crate) complex_class: Rc<RefCell<PyClass>>,
    pub(crate) dict_class: Rc<RefCell<PyClass>>,
    pub(crate) ellipsis_class: Rc<RefCell<PyClass>>,
    pub(crate) float_class: Rc<RefCell<PyClass>>,
    pub(crate) frozenset_class: Rc<RefCell<PyClass>>,
    pub(crate) int_class: Rc<RefCell<PyClass>>,
    pub(crate) list_class: Rc<RefCell<PyClass>>,
    pub(crate) mappingproxy_class: Rc<RefCell<PyClass>>,
    pub(crate) none_class: Rc<RefCell<PyClass>>,
    pub(crate) notimplemented_class: Rc<RefCell<PyClass>>,
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
        let class = Rc::new(RefCell::new(PyClass::new(name, name, base.clone(), attrs)));
        if let Some(b) = base {
            b.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        }
        class
    }
    // Issue #1537: every primitive type inherits from `object` in CPython
    // (`int.__bases__ == (object,)`, etc.).  Setting an explicit `base` here
    // lets `lookup_class_attr` walk to `object` and find dunders like
    // `__init_subclass__`, so `hasattr(int, '__init_subclass__')` returns True.
    // The PRIMITIVE_CLASS_DISPATCH table is keyed on the class pointer (not the
    // base), so the fast-path constructor dispatch is unaffected.
    let obj = object_class_singleton();
    let int_class = make("int", Some(Rc::clone(&obj)));
    let str_class = make("str", Some(Rc::clone(&obj)));
    let list_class = make("list", Some(Rc::clone(&obj)));
    let tuple_class = make("tuple", Some(Rc::clone(&obj)));
    let dict_class = make("dict", Some(Rc::clone(&obj)));
    let set_class = make("set", Some(Rc::clone(&obj)));
    let bytes_class = make("bytes", Some(Rc::clone(&obj)));
    let bytearray_class = make("bytearray", Some(Rc::clone(&obj)));
    populate_primitive_methods(&int_class, "int", INT_METHODS);
    populate_primitive_methods(&bytes_class, "bytes", BYTES_METHODS);
    populate_primitive_methods(&bytearray_class, "bytearray", BYTEARRAY_METHODS);
    populate_primitive_methods(&str_class, "str", STR_METHODS);
    populate_primitive_methods(&list_class, "list", LIST_METHODS);
    populate_primitive_methods(&tuple_class, "tuple", TUPLE_METHODS);
    populate_primitive_methods(&dict_class, "dict", DICT_METHODS);
    populate_primitive_methods(&set_class, "set", SET_METHODS);
    let complex_class = make("complex", Some(Rc::clone(&obj)));
    let frozenset_class = make("frozenset", Some(Rc::clone(&obj)));
    let float_class = make("float", Some(Rc::clone(&obj)));
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
    // `from_bytes` is a classmethod: register it in int_class.attrs so that
    // both `int.from_bytes(b, 'big')` and `(5).from_bytes(b, 'big')` resolve
    // to the same `BuiltinFunction("int.from_bytes")` sentinel.
    int_class
        .borrow_mut()
        .attrs
        .insert("from_bytes".to_string(), Value::builtin_function("int.from_bytes"));
    // Issue #988: register `__init__` on dict/list/set so that
    // `super().__init__()` from a subclass can resolve it via MRO lookup
    // without raising AttributeError.  The registered dispatch returns None
    // (no-op) when called from super() with no args, and populates the
    // backing store when called via invoke_class_method with constructor args.
    // Issue #1004: frozenset and tuple are immutable; their `__init__` is a
    // true no-op (backing data is set at `__new__` time).  Register sentinels
    // so that `super().__init__()` in a subclass resolves without AttributeError.
    for (cls, type_name) in [
        (&list_class, "list"),
        (&dict_class, "dict"),
        (&set_class, "set"),
        (&frozenset_class, "frozenset"),
        (&tuple_class, "tuple"),
    ] {
        let sentinel: &'static str =
            Box::leak(format!("{type_name}.__init__").into_boxed_str());
        cls.borrow_mut()
            .attrs
            .insert("__init__".to_string(), Value::builtin_function(sentinel));
    }
    // Issue #1143: register `tuple.__new__` and `frozenset.__new__` so that
    // `super().__new__(cls, it)` from a tuple/frozenset subclass resolves via
    // MRO lookup to the primitive `__new__` which creates a `PyInstance` with
    // the proper backing store (rather than falling through to `object.__new__`
    // which would create a bare instance without backing data).
    // Issue #1465: same fix for scalar primitives — int/str/float/bytes.
    for (cls, type_name) in [
        (&bytes_class, "bytes"),
        (&float_class, "float"),
        (&frozenset_class, "frozenset"),
        (&int_class, "int"),
        (&str_class, "str"),
        (&tuple_class, "tuple"),
    ] {
        let sentinel: &'static str =
            Box::leak(format!("{type_name}.__new__").into_boxed_str());
        cls.borrow_mut()
            .attrs
            .insert("__new__".to_string(), Value::builtin_function(sentinel));
    }
    // Issue #1134: register `dict.__getitem__` so that `super().__getitem__(key)`
    // from a dict subclass resolves via MRO lookup to a BuiltinFunction sentinel
    // and routes through `super_bound_builtin` → registry dispatch.
    // The same is done for list/tuple/bytes so that subclasses overriding
    // `__getitem__` can call `super().__getitem__(key)` without AttributeError.
    // The sentinels are excluded from the "user override" check in eval_index /
    // eval_slice — they represent the base-class implementation, not an override.
    for (cls, type_name) in [
        (&dict_class, "dict"),
        (&list_class, "list"),
        (&tuple_class, "tuple"),
        (&bytes_class, "bytes"),
    ] {
        let sentinel: &'static str =
            Box::leak(format!("{type_name}.__getitem__").into_boxed_str());
        cls.borrow_mut()
            .attrs
            .insert("__getitem__".to_string(), Value::builtin_function(sentinel));
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
    // `bytes.maketrans` is a staticmethod: register it in bytes_class.attrs so
    // that both `bytes.maketrans(f, t)` and `b''.maketrans(f, t)` resolve to
    // the same `BuiltinFunction("bytes.maketrans")` sentinel.
    bytes_class
        .borrow_mut()
        .attrs
        .insert("maketrans".to_string(), Value::builtin_function("bytes.maketrans"));
    // `bytes.fromhex` is a classmethod: register it in bytes_class.attrs so
    // that both `bytes.fromhex(s)` and `b''.fromhex(s)` resolve to the same
    // `BuiltinFunction("bytes.fromhex")` sentinel.
    bytes_class
        .borrow_mut()
        .attrs
        .insert("fromhex".to_string(), Value::builtin_function("bytes.fromhex"));
    // `bytearray.fromhex` is a classmethod: register it in bytearray_class.attrs.
    bytearray_class
        .borrow_mut()
        .attrs
        .insert("fromhex".to_string(), Value::builtin_function("bytearray.fromhex"));
    // `str.maketrans` is a staticmethod: register it in str_class.attrs so
    // that both `str.maketrans(...)` and `"".maketrans(...)` resolve to the
    // same `BuiltinFunction("str.maketrans")` sentinel.
    str_class
        .borrow_mut()
        .attrs
        .insert("maketrans".to_string(), Value::builtin_function("str.maketrans"));
    // Issue #1256: expose dunder methods on primitive class objects so that
    // `hasattr(int, '__add__')` returns True and `int.__add__(1, 2)` works.
    // Each sentinel registers as `BuiltinFunction("<type>.<dunder>")` in the
    // class attrs and must have a matching entry in the builtin registry
    // (bodies/builtins.rs).
    //
    // Issue #1909: the container/sequence protocol dunders (`__getitem__`,
    // `__setitem__`, `__delitem__`, `__contains__`, `__add__`, `__mul__`,
    // `__len__`) are also registered here so the unbound type-level form
    // (`list.__setitem__(l, 0, 9)`, `list.__add__([1], [2])`) resolves and
    // dispatches through `dispatch_builtin_protocol_dunder`.  The names per
    // type mirror `calls.rs::builtin_protocol_dunders`.
    for (cls, type_name, dunders) in [
        (&int_class, "int", &[
            "__add__", "__sub__", "__mul__", "__truediv__", "__floordiv__",
            "__mod__", "__pow__", "__and__", "__or__", "__xor__",
            "__lshift__", "__rshift__",
            "__lt__", "__le__", "__gt__", "__ge__", "__eq__", "__ne__",
        ][..]),
        (&str_class, "str", &[
            "__len__", "__getitem__", "__contains__", "__add__", "__mul__",
            "__lt__", "__le__", "__gt__", "__ge__", "__eq__", "__ne__",
        ][..]),
        (&list_class, "list", &[
            "__len__", "__getitem__", "__setitem__", "__delitem__",
            "__contains__", "__add__", "__mul__", "__iadd__", "__imul__",
        ][..]),
        (&tuple_class, "tuple", &[
            "__len__", "__getitem__", "__contains__", "__add__", "__mul__",
        ][..]),
        (&dict_class, "dict", &[
            "__len__", "__getitem__", "__setitem__", "__delitem__", "__contains__",
            "__or__", "__ror__", "__ior__",
        ][..]),
        (&set_class, "set", &[
            "__len__", "__contains__", "__or__", "__ror__", "__and__", "__rand__",
            "__sub__", "__rsub__", "__xor__", "__rxor__", "__ior__", "__iand__",
            "__isub__", "__ixor__",
        ][..]),
        (&frozenset_class, "frozenset", &[
            "__len__", "__contains__", "__or__", "__ror__", "__and__", "__rand__",
            "__sub__", "__rsub__", "__xor__", "__rxor__",
        ][..]),
        (&bytes_class, "bytes", &[
            "__len__", "__getitem__", "__contains__", "__add__", "__mul__",
        ][..]),
        (&bytearray_class, "bytearray", &[
            "__len__", "__getitem__", "__setitem__", "__delitem__",
            "__contains__", "__add__", "__mul__", "__iadd__", "__imul__",
        ][..]),
        (&float_class, "float", &["__trunc__", "__floor__", "__ceil__"][..]),
    ] {
        for &dunder in dunders {
            let qualified: &'static str =
                Box::leak(format!("{type_name}.{dunder}").into_boxed_str());
            cls.borrow_mut()
                .attrs
                .insert(dunder.to_string(), Value::builtin_function(qualified));
        }
    }
    PrimitiveClasses {
        bytearray_class,
        bytes_class,
        complex_class,
        dict_class,
        // Issue #2151: NoneType/NotImplementedType/ellipsis/mappingproxy must
        // inherit from `object` like every other primitive, so that the object
        // dunders (`__eq__`, `__str__`, `__repr__`, `__hash__`, `__doc__`,
        // `__sizeof__`, …) resolve for `None`/`NotImplemented`/`...`.  Without
        // an explicit base these classes ended their MRO at themselves and
        // exposed *no* dunders at all (`hasattr(None, '__eq__')` was False).
        ellipsis_class: make("ellipsis", Some(Rc::clone(&obj))),
        float_class,
        frozenset_class,
        list_class,
        mappingproxy_class: make("mappingproxy", Some(Rc::clone(&obj))),
        none_class: {
            let c = make("NoneType", Some(Rc::clone(&obj)));
            // `None.__bool__()` returns False; `__bool__` is NoneType-specific
            // (not inherited from `object`), so register it on the class.
            c.borrow_mut()
                .attrs
                .insert("__bool__".to_string(), Value::builtin_function("NoneType.__bool__"));
            c
        },
        notimplemented_class: make("NotImplementedType", Some(Rc::clone(&obj))),
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
const INT_METHODS: &[&str] = &[
    "bit_length",
    "bit_count",
    "conjugate",
    "is_integer",
    "to_bytes",
    "as_integer_ratio",
];
const BYTES_METHODS: &[&str] = pyrust_builtins::bytes::METHODS;
const BYTEARRAY_METHODS: &[&str] = pyrust_builtins::bytearray::METHODS;

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
    "translate",
];

const LIST_METHODS: &[&str] = &[
    "index", "count",
    "append", "clear", "copy", "extend", "insert", "pop", "remove", "reverse",
    "sort",
];

const TUPLE_METHODS: &[&str] = &["index", "count"];

// `fromkeys` is a classmethod registered via `populate_primitive_methods`
// so that `BuiltinFunction("dict.fromkeys")` ends up in the dict class
// attrs.  It is dispatched through the builtin registry (see
// `builtins.rs::dict_fromkeys`), NOT via `dict::call` / `call_dict_method`.
const DICT_METHODS: &[&str] = &[
    "fromkeys", "get", "keys", "values", "items", "update", "pop", "popitem", "clear",
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
    OBJECT_CLASS.with(Rc::clone)
}

/// Returns the singleton `type` metaclass.  In CPython `type(int)` returns
/// `<class 'type'>` and `isinstance(int, type)` is `True` because every class
/// is an instance of `type` (the metaclass).  Using a per-thread singleton
/// mirrors the `object_class_singleton` pattern (issue #1312).
pub(crate) fn type_class_singleton() -> Rc<RefCell<PyClass>> {
    TYPE_CLASS.with(Rc::clone)
}

/// Returns the metaclass (metatype) of `class`.  A class with no explicit
/// metatype (the common case) is an instance of the built-in `type`
/// singleton, so this returns the `type` singleton in that case.
/// Issues #1955/#1956/#1960.
pub(crate) fn metaclass_of(class: &Rc<RefCell<PyClass>>) -> Rc<RefCell<PyClass>> {
    class
        .borrow()
        .metatype
        .clone()
        .unwrap_or_else(type_class_singleton)
}

/// Look up attribute `name` on the metaclass MRO of `class`, returning it only
/// when it is a *user* override — i.e. the metaclass is something other than
/// the built-in `type` singleton and the attribute is found before the walk
/// reaches `type`/`object`.  Returns `None` for ordinary classes (metatype is
/// `type`), so plain `Cls()` / `Cls.attr` / `isinstance` keep their fast
/// paths and never recurse into the default `type` slot.  Used for both
/// metaclass dunder hooks (`__call__` / `__instancecheck__` / `__getattr__`)
/// and plain metaclass attributes reached via `cls.attr`.
/// Issues #1955/#1956/#1960.
pub(crate) fn metaclass_dunder(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    let meta = class.borrow().metatype.clone()?;
    // A metatype that is the `type` singleton itself has no user override.
    if Rc::ptr_eq(&meta, &type_class_singleton()) {
        return None;
    }
    lookup_user_metaclass_attr(&meta, name)
}

/// Walk `meta`'s MRO looking for `name`, but stop short of the built-in
/// `type` and `object` singletons — those carry the *default* slots, which
/// must not be treated as user overrides (that would defeat the fast path
/// and risk infinite recursion in `type.__call__` chaining).
fn lookup_user_metaclass_attr(meta: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    if Rc::ptr_eq(meta, &type_class_singleton()) || Rc::ptr_eq(meta, &object_class_singleton()) {
        return None;
    }
    let (value, base, extra_bases) = {
        let borrowed = meta.borrow();
        (borrowed.attrs.get(name).cloned(), borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    if value.is_some() {
        return value;
    }
    if let Some(base) = base {
        if let Some(v) = lookup_user_metaclass_attr(&base, name) {
            return Some(v);
        }
    }
    for extra in &extra_bases {
        if let Some(v) = lookup_user_metaclass_attr(extra, name) {
            return Some(v);
        }
    }
    None
}

/// Returns the singleton `method` class.  In CPython, `type(instance.method)`
/// returns `<class 'method'>` and `type(type(c.m)) is type` holds because
/// `method` is a proper `PyClass` (not a `BuiltinFunction` sentinel).
/// Issue #1528.
pub(crate) fn method_type_singleton() -> Rc<RefCell<PyClass>> {
    METHOD_TYPE.with(Rc::clone)
}

/// Returns the singleton `function` class.  In CPython, `type(lambda: None)`
/// returns `<class 'function'>` and `type(type(lambda: None)) is type` holds.
/// Issue #1528.
pub(crate) fn function_type_singleton() -> Rc<RefCell<PyClass>> {
    FUNCTION_TYPE.with(Rc::clone)
}

/// Returns the singleton `range` class.  In CPython, `range` is a proper
/// type (`type(range(5)) is range`), not a builtin function.  This singleton
/// is registered in `PRIMITIVE_CLASS_DISPATCH` so that calling
/// `range(start, stop)` still dispatches to the existing registry fn, and
/// is linked into ABC `extra_bases` so `issubclass(range, Sequence)` works.
/// Issues #1793, #1800.
pub(crate) fn range_class_singleton() -> Rc<RefCell<PyClass>> {
    RANGE_CLASS.with(Rc::clone)
}

/// Look up the per-primitive `PyClass` singleton for one of the migrated
/// primitive type names (`int`, `str`, `list`, …).  Returns `None` for any
/// other name — callers fall through to the legacy `BuiltinFunction(name)`
/// path.  See [`PRIMITIVE_CLASSES`].
pub(crate) fn primitive_class_by_name(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    if name == "range" {
        return Some(RANGE_CLASS.with(Rc::clone));
    }
    PRIMITIVE_CLASSES.with(|c| {
        Some(Rc::clone(match name {
            "bool" => &c.bool_class,
            "bytearray" => &c.bytearray_class,
            "bytes" => &c.bytes_class,
            "complex" => &c.complex_class,
            "dict" => &c.dict_class,
            "ellipsis" => &c.ellipsis_class,
            "float" => &c.float_class,
            "frozenset" => &c.frozenset_class,
            "int" => &c.int_class,
            "list" => &c.list_class,
            "mappingproxy" => &c.mappingproxy_class,
            "NoneType" => &c.none_class,
            "NotImplementedType" => &c.notimplemented_class,
            "set" => &c.set_class,
            "str" => &c.str_class,
            "tuple" => &c.tuple_class,
            _ => return None,
        }))
    })
}

/// Return the `PyClass` that `type(v)` should yield for primitive types.
/// Returns `None` for variants that aren't part of this migration (functions,
/// modules, instances, …) — the caller falls back to its existing per-variant
/// logic.
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
        ValueKind::None => "NoneType",
        ValueKind::NotImplemented => "NotImplementedType",
        ValueKind::Ellipsis => "ellipsis",
        ValueKind::Range { .. } | ValueKind::BigRange { .. } => {
            return Some(RANGE_CLASS.with(Rc::clone));
        }
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "bytearray" => "bytearray",
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

/// Constructor for `NoneType` (issue #1451).
///
/// CPython 3.12: `type(None)()` returns `None`; any arguments raise
/// `TypeError: NoneType takes no arguments`.
fn none_ctor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "NoneType takes no arguments".to_string(),
        ));
    }
    Ok(Value::none())
}

/// Constructor for `NotImplementedType` (issue #1451).
///
/// CPython 3.12: `type(NotImplemented)()` returns `NotImplemented`; any
/// arguments raise `TypeError: NotImplementedType takes no arguments`.
fn notimplemented_ctor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "NotImplementedType takes no arguments".to_string(),
        ));
    }
    Ok(Value::not_implemented())
}

/// Constructor for `ellipsis` (issue #1451).
///
/// CPython 3.12: `type(...)()` returns `Ellipsis`; any arguments raise
/// `TypeError: EllipsisType takes no arguments`.
fn ellipsis_ctor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "EllipsisType takes no arguments".to_string(),
        ));
    }
    Ok(Value::ellipsis())
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
        if cls_ptr == Rc::as_ptr(&c.none_class) {
            return Some(matches!(obj.kind(), ValueKind::None));
        }
        if cls_ptr == Rc::as_ptr(&c.notimplemented_class) {
            return Some(matches!(obj.kind(), ValueKind::NotImplemented));
        }
        if cls_ptr == Rc::as_ptr(&c.ellipsis_class) {
            return Some(matches!(obj.kind(), ValueKind::Ellipsis));
        }
        None
    })
}

/// `isinstance(v, (str, bytes, bytearray, dict, set, frozenset))` — the set of
/// types PEP 634 §3 excludes from sequence-pattern matching.
///
/// Replicates [`isinstance_single`]'s semantics for those primitive classes
/// (including subclass instances) without building a tuple of type objects or
/// going through the generic `isinstance` call.  Backs the `MatchSeqExcluded`
/// instruction so a `match` arm with a sequence pattern pays one
/// allocation-free check per execution instead of rebuilding the exclusion
/// tuple every time (issue #1789).
///
/// `bytearray` is excluded (issue #1844): although it is a mutable byte buffer,
/// CPython does not set `Py_TPFLAGS_SEQUENCE` on it, so `match bytearray(b"ab")`
/// against `case [a, b]` is a no-match.
pub(crate) fn value_is_seq_excluded(v: &Value) -> bool {
    match v.kind() {
        // Direct primitives — the common case, decided by the NaN-box tag.
        ValueKind::Str(_) | ValueKind::Bytes(_) | ValueKind::Dict(_) | ValueKind::Set(_) => true,
        ValueKind::BuiltinObject { ops, .. } => {
            let name = ops.type_name();
            name == "frozenset" || name == pyrust_builtins::bytearray::TYPE_NAME
        }
        // Subclass instances (`class MyDict(dict)`): walk the MRO against each
        // excluded primitive singleton, matching `isinstance(_, dict)` etc.
        ValueKind::PyInstance(inst) => {
            let actual = Rc::clone(&inst.borrow().class);
            PRIMITIVE_CLASSES.with(|c| {
                class_is_subclass_of(&actual, &c.str_class)
                    || class_is_subclass_of(&actual, &c.bytes_class)
                    || class_is_subclass_of(&actual, &c.bytearray_class)
                    || class_is_subclass_of(&actual, &c.dict_class)
                    || class_is_subclass_of(&actual, &c.set_class)
                    || class_is_subclass_of(&actual, &c.frozenset_class)
            })
        }
        _ => false,
    }
}

/// True if `v` is a mapping for the purposes of `match` mapping patterns
/// (`case {k: p}`).  PEP 634 §3 gates the whole mapping pattern on
/// `isinstance(subject, collections.abc.Mapping)`; a non-mapping subject
/// silently fails to match instead of raising.  In pyrust the only built-in
/// mapping is `dict` (and its subclasses), so this is `isinstance(v, dict)`
/// without building a tuple or invoking the generic `isinstance` path.
/// Backs the `MatchMapping` instruction (issue #1879), mirroring how
/// `value_is_seq_excluded` backs `MatchSeqExcluded` for sequence patterns.
pub(crate) fn value_is_mapping(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Dict(_) => true,
        // `mappingproxy` (e.g. `type(C).__dict__`) is registered as a
        // `collections.abc.Mapping` in CPython, so it matches a mapping
        // pattern (issue #1879).  Like the `BuiltinObject` arm in
        // `value_is_seq_excluded`, decide on the type name.
        ValueKind::BuiltinObject { ops, .. } => {
            ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME
        }
        // Subclass instances (`class MyDict(dict)`): walk the MRO against the
        // dict singleton, matching `isinstance(_, dict)`.
        ValueKind::PyInstance(inst) => {
            let actual = Rc::clone(&inst.borrow().class);
            PRIMITIVE_CLASSES.with(|c| class_is_subclass_of(&actual, &c.dict_class))
        }
        _ => false,
    }
}

/// Returns the type name if `class` is one of the builtin types that
/// CPython marks as non-subclassable (i.e. lacks `Py_TPFLAGS_BASETYPE`):
/// `NoneType`, `ellipsis`, `NotImplementedType`, `bool`, `method`, and
/// `function`.
///
/// Used in the `MakeClass` instruction to raise `TypeError: type 'X' is
/// not an acceptable base type` before the class body runs.
pub(crate) fn non_subclassable_builtin_name(
    class: &Rc<RefCell<PyClass>>,
) -> Option<&'static str> {
    let ptr = Rc::as_ptr(class);
    // Check the METHOD_TYPE and FUNCTION_TYPE singletons (issue #1528): CPython
    // raises `TypeError: type 'method'/'function' is not an acceptable base type`
    // when either is used as a base class.
    if METHOD_TYPE.with(|m| ptr == Rc::as_ptr(m)) {
        return Some("method");
    }
    if FUNCTION_TYPE.with(|f| ptr == Rc::as_ptr(f)) {
        return Some("function");
    }
    // Issues #1793, #1800: RANGE_CLASS is a proper PyClass singleton so that
    // `issubclass(range, Sequence)` works, but `range` is not subclassable in
    // CPython (`TypeError: type 'range' is not an acceptable base type`).
    if RANGE_CLASS.with(|r| ptr == Rc::as_ptr(r)) {
        return Some("range");
    }
    PRIMITIVE_CLASSES.with(|c| {
        if ptr == Rc::as_ptr(&c.none_class) {
            return Some("NoneType");
        }
        if ptr == Rc::as_ptr(&c.notimplemented_class) {
            return Some("NotImplementedType");
        }
        if ptr == Rc::as_ptr(&c.ellipsis_class) {
            return Some("ellipsis");
        }
        if ptr == Rc::as_ptr(&c.bool_class) {
            return Some("bool");
        }
        None
    })
}

/// Returns `true` if `class` is one of the built-in types that carry a
/// non-trivial C-level instance layout (`int`, `str`, `float`, `bytes`,
/// `tuple`, `list`, `dict`, `set`, `frozenset`).  CPython raises
/// `TypeError: multiple bases have instance lay-out conflict` when two or
/// more such types appear in the same bases tuple.  Issue #1677.
pub(crate) fn is_solid_primitive_class(class: &Rc<RefCell<PyClass>>) -> bool {
    let ptr = Rc::as_ptr(class);
    PRIMITIVE_CLASSES.with(|c| {
        ptr == Rc::as_ptr(&c.int_class)
            || ptr == Rc::as_ptr(&c.str_class)
            || ptr == Rc::as_ptr(&c.float_class)
            || ptr == Rc::as_ptr(&c.bytes_class)
            || ptr == Rc::as_ptr(&c.tuple_class)
            || ptr == Rc::as_ptr(&c.list_class)
            || ptr == Rc::as_ptr(&c.dict_class)
            || ptr == Rc::as_ptr(&c.set_class)
            || ptr == Rc::as_ptr(&c.frozenset_class)
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

/// Walk the base chain of `class` and return the name of the first scalar
/// (non-container) primitive builtin base found (`"str"`, `"int"`, `"float"`,
/// `"bytes"`, or `"complex"`), or `None` if the class does not inherit from
/// any of these.
///
/// Issue #1204: these types require the same `__builtin_data__` backing-store
/// approach used by the container primitives (`dict`/`list`/`set`), so that
/// subclass instances can delegate method dispatch to the underlying primitive
/// value.  Like `find_immutable_primitive_base`, the backing is populated at
/// construction time from the constructor args and is fixed thereafter.
pub(crate) fn find_scalar_primitive_base(
    class: &Rc<RefCell<PyClass>>,
) -> Option<&'static str> {
    let (name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    match name.as_str() {
        "str" | "int" | "float" | "bytes" | "bytearray" | "complex" => {
            if is_primitive_class(class) {
                return Some(match name.as_str() {
                    "str" => "str",
                    "int" => "int",
                    "float" => "float",
                    "bytes" => "bytes",
                    "bytearray" => "bytearray",
                    "complex" => "complex",
                    _ => unreachable!(),
                });
            }
        }
        _ => {}
    }
    base.and_then(|b| find_scalar_primitive_base(&b))
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

/// Returns `true` if `v` is a `str` value or a `str` subclass instance.
///
/// CPython's `__format__` protocol accepts `str` subclasses as valid return
/// values (they satisfy `isinstance(result, str)`).  A subclass instance is
/// represented as a `PyInstance` whose `__builtin_data__` backing is `Str`.
pub(crate) fn is_str_or_str_subclass(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Str(_) => true,
        ValueKind::PyInstance(inst) => matches!(
            inst.borrow().attrs.get(BUILTIN_DATA_ATTR).map(|b| b.kind()),
            Some(ValueKind::Str(_))
        ),
        _ => false,
    }
}

/// Extract the string content from a value that is known to satisfy
/// `is_str_or_str_subclass`.  Returns the backing `String` so the caller
/// can append it without holding a borrow across a `RefCell`.
///
/// Panics (debug) if called on a value that is neither `Str` nor a
/// `PyInstance` with a `Str` backing — callers must gate on
/// `is_str_or_str_subclass` first.
pub(crate) fn extract_str_value(v: &Value) -> String {
    match v.kind() {
        ValueKind::Str(s) => s.to_string(),
        ValueKind::PyInstance(inst) => {
            let borrowed = inst.borrow();
            if let Some(backing) = borrowed.attrs.get(BUILTIN_DATA_ATTR) {
                if let ValueKind::Str(s) = backing.kind() {
                    return s.to_string();
                }
            }
            debug_assert!(false, "extract_str_value called on non-str instance");
            v.to_py_str()
        }
        _ => {
            debug_assert!(false, "extract_str_value called on non-str value");
            v.to_py_str()
        }
    }
}

/// Coerce a `str`-subclass instance argument to its backing `Str` value so the
/// receiver-only `pyrust_builtins::string` arg extractors (which match an exact
/// `ValueKind::Str`) accept it — CPython's `str` methods accept any `str`
/// subclass argument (an `isinstance` relationship), #1927.
///
/// The common case (an exact `str`, or any non-`PyInstance` value) is returned
/// untouched after a single cheap tag check, so genuinely-wrong-type arguments
/// still reach the extractor and raise the existing `TypeError`.  Only a
/// `PyInstance` whose `__builtin_data__` backing is `Str` is rewritten.
pub(crate) fn coerce_str_subclass_arg(v: Value) -> Value {
    let backing = if let ValueKind::PyInstance(inst) = v.kind() {
        inst.borrow()
            .attrs
            .get(BUILTIN_DATA_ATTR)
            .filter(|b| matches!(b.kind(), ValueKind::Str(_)))
            .cloned()
    } else {
        None
    };
    backing.unwrap_or(v)
}

/// Coerce a `bytes`-subclass instance argument to its backing `Bytes` value so
/// the `pyrust_builtins::bytes` arg extractors (which match an exact
/// `ValueKind::Bytes`) accept it — CPython's `bytes`/`bytearray` methods accept
/// any `bytes` subclass argument, #1928.
///
/// Like [`coerce_str_subclass_arg`], the common case is returned untouched
/// after a single tag check; only a `PyInstance` with a `Bytes` backing (a
/// `bytes` subclass) or a `bytearray` is rewritten to a real `Bytes` value.
/// CPython treats both as bytes-like objects accepted by `bytes` methods.
pub(crate) fn coerce_bytes_subclass_arg(v: Value) -> Value {
    enum Kind {
        Instance,
        Builtin,
        Other,
    }
    let kind = match v.kind() {
        ValueKind::PyInstance(_) => Kind::Instance,
        ValueKind::BuiltinObject { .. } => Kind::Builtin,
        _ => Kind::Other,
    };
    match kind {
        Kind::Instance => {
            let backing = v.as_py_instance_rc().and_then(|inst| {
                inst.borrow()
                    .attrs
                    .get(BUILTIN_DATA_ATTR)
                    .filter(|b| matches!(b.kind(), ValueKind::Bytes(_)))
                    .cloned()
            });
            backing.unwrap_or(v)
        }
        Kind::Builtin => match pyrust_builtins::bytearray::as_bytearray_snapshot(&v) {
            Some(snapshot) => Value::bytes(snapshot),
            None => v,
        },
        Kind::Other => v,
    }
}

/// Coerce a `startswith`/`endswith` first argument, which may be either a
/// single prefix/suffix or a *tuple* of them.  A tuple has each element coerced
/// (and is rebuilt); any other value is coerced directly via `coerce` (a no-op
/// for non-subclass values).  Shared by the str and bytes coercion paths.
fn coerce_prefix_arg(v: Value, coerce: fn(Value) -> Value) -> Value {
    let tuple_items: Option<Vec<Value>> = match v.kind() {
        ValueKind::Tuple(items) => Some(items.to_vec()),
        _ => None,
    };
    match tuple_items {
        Some(items) => Value::tuple(items.into_iter().map(coerce).collect()),
        None => coerce(v),
    }
}

/// Coerce the positional arguments of a `str` method so str-subclass instances
/// are accepted (#1927).  Every top-level argument is run through
/// [`coerce_str_subclass_arg`] (a no-op for the common exact-str / int / None
/// cases).  For `startswith`/`endswith` the first argument may be a *tuple* of
/// prefixes; its elements are coerced too.
pub(crate) fn coerce_str_subclass_method_args(method: &str, mut args: Vec<Value>) -> Vec<Value> {
    // Hot path: the overwhelmingly common case is exact-str (or int / None)
    // arguments with no `PyInstance` and no tuple to descend into.  Bail out
    // after a single scan so a normal `"x".count("y")` pays nothing beyond it —
    // no per-element coercion, no Vec rebuild.
    if !args
        .iter()
        .any(|a| matches!(a.kind(), ValueKind::PyInstance(_) | ValueKind::Tuple(_)))
    {
        return args;
    }
    let tuple_arg0 = matches!(method, "startswith" | "endswith");
    for (i, a) in args.iter_mut().enumerate() {
        let taken = std::mem::replace(a, Value::none());
        *a = if tuple_arg0 && i == 0 {
            coerce_prefix_arg(taken, coerce_str_subclass_arg)
        } else {
            coerce_str_subclass_arg(taken)
        };
    }
    args
}

/// Coerce the positional arguments of a `bytes`/`bytearray` method so
/// bytes-subclass and bytearray instances are accepted (#1928).  Mirror of
/// [`coerce_str_subclass_method_args`].
pub(crate) fn coerce_bytes_subclass_method_args(
    method: &str,
    mut args: Vec<Value>,
) -> Vec<Value> {
    // Hot path: exact-bytes / int args need no coercion.  A bytes-subclass is a
    // `PyInstance`; a bytearray is a `BuiltinObject`; the tuple form is a
    // `Tuple`.  Anything else (Bytes, Int, Bool, …) is left untouched.
    if !args.iter().any(|a| {
        matches!(
            a.kind(),
            ValueKind::PyInstance(_) | ValueKind::BuiltinObject { .. } | ValueKind::Tuple(_)
        )
    }) {
        return args;
    }
    let tuple_arg0 = matches!(method, "startswith" | "endswith");
    for (i, a) in args.iter_mut().enumerate() {
        let taken = std::mem::replace(a, Value::none());
        *a = if tuple_arg0 && i == 0 {
            coerce_prefix_arg(taken, coerce_bytes_subclass_arg)
        } else {
            coerce_bytes_subclass_arg(taken)
        };
    }
    args
}

/// Coerce the elements of a `join` iterable (`str.join`) so str-subclass items
/// join by their str value (#1927).  Only `List`/`Tuple` fast-path containers
/// are rewritten, and only when an element actually needs coercing — an
/// all-exact-str container is returned untouched after a scan.  Any other
/// iterable kind is returned unchanged for the builtins join fn to handle.
pub(crate) fn coerce_str_subclass_join_iterable(iterable: Value) -> Value {
    let needs_coerce = |v: &Value| {
        matches!(v.kind(), ValueKind::PyInstance(inst)
            if matches!(
                inst.borrow().attrs.get(BUILTIN_DATA_ATTR).map(|b| b.kind()),
                Some(ValueKind::Str(_))
            ))
    };
    // Scan the elements *under the borrow* without cloning the container — the
    // all-exact-str case (overwhelmingly common) then pays only the scan and
    // returns `iterable` untouched.  Only when an element actually needs
    // coercing do we snapshot and rebuild a coerced list.
    let snapshot: Option<Vec<Value>> = match iterable.kind() {
        ValueKind::List(items) => {
            if !items.iter().any(needs_coerce) {
                None
            } else {
                Some(items.iter().cloned().collect())
            }
        }
        ValueKind::Tuple(items) => {
            if !items.iter().any(needs_coerce) {
                None
            } else {
                Some(items.to_vec())
            }
        }
        _ => None,
    };
    match snapshot {
        Some(items) => Value::list(items.into_iter().map(coerce_str_subclass_arg).collect()),
        None => iterable,
    }
}

/// Coerce the elements of a `bytes.join` iterable so bytes-subclass / bytearray
/// items join by their bytes value (#1928).  Mirror of
/// [`coerce_str_subclass_join_iterable`].
pub(crate) fn coerce_bytes_subclass_join_iterable(iterable: Value) -> Value {
    let snapshot: Option<Vec<Value>> = match iterable.kind() {
        ValueKind::List(items) => Some(items.iter().cloned().collect()),
        ValueKind::Tuple(items) => Some(items.to_vec()),
        _ => None,
    };
    let Some(items) = snapshot else {
        return iterable;
    };
    let needs = items.iter().any(|v| match v.kind() {
        ValueKind::PyInstance(inst) => matches!(
            inst.borrow().attrs.get(BUILTIN_DATA_ATTR).map(|b| b.kind()),
            Some(ValueKind::Bytes(_))
        ),
        ValueKind::BuiltinObject { ops, .. } => ops.type_name() == "bytearray",
        _ => false,
    });
    if !needs {
        return iterable;
    }
    Value::list(items.into_iter().map(coerce_bytes_subclass_arg).collect())
}

pub(crate) struct PrintOptions {
    pub(crate) values: Vec<Value>,
    pub(crate) sep: String,
    pub(crate) end: String,
    /// `None` means write to stdout; `Some(v)` means call `v.write(...)`.
    pub(crate) file: Option<Value>,
    /// When true and `file` is `Some`, call `file.flush()` after writing.
    pub(crate) flush: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ExpandedCallArg {
    pub(crate) name: Option<String>,
    pub(crate) value: Value,
}

/// Inline-4 buffer for building small call-arg slices without heap allocation.
/// Covers `self + 0..3 args` — the dominant case for method invocations.
pub(crate) type ExpandedArgBuf = smallvec::SmallVec<[ExpandedCallArg; 4]>;

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
            return interp.call_user_function_expanded(func, args, &[instance]);
        }
        ValueKind::BuiltinFunction(name) => {
            // Issue #1909: container protocol-dunder sentinels
            // (`dict.__contains__`, `list.__setitem__`, …) registered on the
            // primitive class objects have no registry body — they dispatch
            // through the operator machinery.  Route them here (covering the
            // implicit `in` / `[]` operator dispatch on a primitive *subclass*
            // and `super().__contains__(...)` calls) before the registry probe.
            if let Some((type_name, method)) = name.split_once('.') {
                if method.starts_with("__")
                    && builtin_protocol_dunders(type_name).contains(&method)
                {
                    // Resolve the receiver to its backing primitive when the
                    // instance is a builtin-subclass PyInstance; a plain
                    // primitive (super() from a non-subclass) is used directly.
                    let receiver = match instance.kind() {
                        ValueKind::PyInstance(inst) => {
                            instance_builtin_data(inst).unwrap_or_else(|| instance.clone())
                        }
                        _ => instance.clone(),
                    };
                    let method = method.to_string();
                    let rest: Vec<Value> = args
                        .iter()
                        .filter(|a| a.name.is_none())
                        .map(|a| a.value.clone())
                        .collect();
                    return interp.dispatch_builtin_protocol_dunder(&method, receiver, rest);
                }
            }
            // PEP 654: BaseExceptionGroup.derive / subgroup / split are not
            // registry builtins (they need interpreter access for predicates and
            // a subclass's overridden `derive`).  Dispatch them here with the
            // receiver prepended.
            if matches!(
                name,
                "BaseExceptionGroup.derive"
                    | "BaseExceptionGroup.subgroup"
                    | "BaseExceptionGroup.split"
            ) {
                let mut combined: Vec<ExpandedCallArg> = Vec::with_capacity(args.len() + 1);
                combined.push(ExpandedCallArg { name: None, value: instance.clone() });
                combined.extend(args.iter().cloned());
                return match name {
                    "BaseExceptionGroup.derive" => interp.exception_group_derive(&combined),
                    "BaseExceptionGroup.subgroup" => {
                        interp.exception_group_subgroup_or_split(&combined, false)
                    }
                    _ => interp.exception_group_subgroup_or_split(&combined, true),
                };
            }
            let dispatch = crate::builtin_registry::lookup(name).ok_or_else(|| {
                PyError::Runtime(format!(
                    "internal: builtin method '{name}' not in registry"
                ))
            })?;
            // Reuse the interpreter-level buffer to eliminate a per-invocation
            // heap allocation on the hot dunder dispatch path.  `std::mem::take`
            // leaves an empty SmallVec in `interp.invoke_arg_buf`; on recursive
            // re-entry the field is already empty so a fresh SmallVec is used
            // only for the nested call.  The buffer is always restored after
            // dispatch (both Ok and Err paths).
            let mut combined = std::mem::take(&mut interp.invoke_arg_buf);
            combined.clear();
            combined.push(ExpandedCallArg {
                name: None,
                value: instance,
            });
            combined.extend(args.iter().cloned());
            let result = dispatch(interp, &combined);
            interp.invoke_arg_buf = combined;
            return result;
        }
        // The non-function arm is handled after the match so `method_val` can be
        // moved out of the borrow taken by `method_val.kind()` above.
        _ => {}
    }
    // Issue #2054: the resolved slot is not a plain function but may still be
    // callable — a bound method, a class object, or a callable *instance* (an
    // object whose class defines `__call__`).  CPython invokes whatever the slot
    // resolves to.  Such a slot is *not* a descriptor, so (unlike a function
    // slot) it does NOT receive the receiver as `self`: `__len__ = Caller()`
    // calls `Caller()()` with no implicit self, `__add__ = Caller()` calls
    // `Caller()(other)`.  Route through the normal call machinery with `args`.
    //
    // Genuinely non-callable slots (`Foo.__len__ = 5`) raise the standard
    // "object is not callable" keyed on the *resolved value's* type, not the
    // owning class: `len(D())` with `__len__ = 5` -> "'int' object is not
    // callable".  Match that exactly so every implicit-dunder dispatch path
    // agrees with CPython 3.12 (issue #1963 / #2055).
    if slot_is_callable(&method_val) {
        interp.call_function_expanded(method_val, args)
    } else {
        Err(PyError::named(
            "TypeError",
            format!("'{}' object is not callable", value_type_name_str(&method_val)),
        ))
    }
}

/// If `value` is a `PyInstance` whose class exposes a `keys` method, treat it
/// as a mapping per CPython's mapping protocol (`dict(m)` / `{**m}` /
/// `dict.update(m)` all key on `keys()` + `__getitem__`) and materialise its
/// `(PyKey, Value)` pairs.  Returns `Ok(None)` for any value that is not a
/// `keys()`-bearing instance, so callers fall back to their iterable-of-pairs
/// path unchanged.
///
/// This covers `collections.ChainMap`, `UserDict`/`OrderedDict` subclasses, and
/// any user class that follows the duck-typed mapping protocol — without
/// requiring a concrete builtin `dict` backing (issue #2190).
pub(crate) fn mapping_pairs_via_protocol(
    interp: &mut Interpreter,
    value: &Value,
) -> Result<Option<Vec<(PyKey, Value)>>> {
    let inst = match value.kind() {
        ValueKind::PyInstance(inst) => Rc::clone(inst),
        _ => return Ok(None),
    };
    let class = Rc::clone(&inst.borrow().class);
    // `dict` subclasses (OrderedDict / defaultdict / Counter): their `keys`
    // is a *builtin* method that is not present in `class.attrs`, so the
    // user-method `lookup_class_attr("keys")` below returns `None`.  Mirror
    // the `dict()` constructor's subclass handling so `{**subclass}`
    // materialises the same pairs as `dict(subclass)` (issue #2190).
    let getitem = lookup_class_attr(&class, "__getitem__");
    let is_dict_subclass = primitive_class_by_name("dict")
        .is_some_and(|dict_class| class_is_subclass_of(&class, &dict_class));
    if is_dict_subclass {
        // Concrete builtin backing dict (e.g. OrderedDict) — extract directly.
        if let Some(backing) = instance_builtin_data(&inst) {
            if let Some(map) = backing.as_dict() {
                return Ok(Some(map.clone().into_iter().collect()));
            }
        }
        // No builtin backing (defaultdict / Counter): iterate the instance for
        // its keys and subscript via `__getitem__`, exactly as `dict()` does.
        let getitem = match getitem {
            Some(m) => m,
            None => return Ok(None),
        };
        let keys = interp.collect_iterable(value)?;
        let mut pairs: Vec<(PyKey, Value)> = Vec::with_capacity(keys.len());
        for k in keys {
            let v = invoke_class_method(
                interp,
                getitem.clone(),
                Value::py_instance(Rc::clone(&inst)),
                &[ExpandedCallArg { name: None, value: k.clone() }],
            )?;
            let key = interp.value_to_pykey(&k)?;
            pairs.push((key, v));
        }
        return Ok(Some(pairs));
    }
    let keys_method = match lookup_class_attr(&class, "keys") {
        Some(m) => m,
        None => return Ok(None),
    };
    let getitem = match getitem {
        Some(m) => m,
        None => return Ok(None),
    };
    // Call `m.keys()` and iterate the result (CPython does not require it to be
    // a list — any iterable of keys is accepted).
    let keys_iter = invoke_class_method(
        interp,
        keys_method,
        Value::py_instance(Rc::clone(&inst)),
        &[],
    )?;
    let keys = interp.collect_iterable(&keys_iter)?;
    let mut pairs: Vec<(PyKey, Value)> = Vec::with_capacity(keys.len());
    for k in keys {
        let v = invoke_class_method(
            interp,
            getitem.clone(),
            Value::py_instance(Rc::clone(&inst)),
            &[ExpandedCallArg { name: None, value: k.clone() }],
        )?;
        let key = interp.value_to_pykey(&k)?;
        pairs.push((key, v));
    }
    Ok(Some(pairs))
}

/// `true` if `value` is a mapping for printf-style `%`-formatting (issue #2089).
///
/// CPython enters mapping mode for a `%(key)` format when the rhs is not a
/// `tuple` and not a `str` and passes `PyMapping_Check` (has `mp_subscript`).
/// A plain `dict` always qualifies; a `PyInstance` qualifies when it exposes
/// `__getitem__` and is not a `tuple` or `str` subclass (`list`/`bytes`/
/// `bytearray`/`range` subclasses and custom `__getitem__` classes do qualify,
/// matching CPython — the subscript itself then raises the type-appropriate
/// error for a non-mapping key).
pub(crate) fn is_percent_format_mapping(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Dict(_) => true,
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__getitem__").is_some()
                && find_immutable_primitive_base(&class) != Some("tuple")
                && find_scalar_primitive_base(&class) != Some("str")
        }
        _ => false,
    }
}

fn extract_optional_string(value: Value, name: &str) -> Result<Option<String>> {
    match value.kind() {
        ValueKind::Str(text) => Ok(Some(text.to_string())),
        ValueKind::None => Ok(None),
        _ => Err(PyError::named(
            "TypeError",
            format!("{} must be None or a string, not {}", name, value_type_name_str(&value)),
        )),
    }
}

pub(crate) fn reject_keyword_args_expanded(function_name: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.iter().any(|a| a.name.is_some()) {
        // CPython raises TypeError with "takes no keyword arguments" when a
        // builtin accepts no keyword arguments at all (not a specific-kwarg
        // rejection).  Match that wording for parity.
        return Err(PyError::named(
            "TypeError",
            format!("{function_name}() takes no keyword arguments"),
        ));
    }
    Ok(())
}

/// Bind a builtin constructor's call args (positional + keyword) into a
/// per-parameter slot vector in declared parameter order, matching CPython
/// 3.12's argument-binding error semantics.
///
/// `params` lists every parameter in positional order; `keyword_ok` is the
/// matching mask of whether each parameter is keyword-acceptable (a `false`
/// entry is positional-only — supplying it by name yields the CPython
/// `'<name>' is an invalid keyword argument for <fn>()` error).  `max_args`
/// is the constructor's maximum total arity used for the "takes at most N
/// arguments" overflow check, which CPython performs *before* validating
/// keyword names.
///
/// Returns one slot per declared parameter (`None` for an unfilled slot), so
/// each constructor can apply its own per-parameter defaults / arity logic.
pub(crate) fn bind_constructor_kwargs(
    function_name: &str,
    args: &[ExpandedCallArg],
    params: &[&str],
    keyword_ok: &[bool],
    max_args: usize,
) -> Result<Vec<Option<Value>>> {
    debug_assert_eq!(params.len(), keyword_ok.len());

    // CPython checks total arity before validating individual keyword names:
    // `complex(1, 2, foo=3)` reports "takes at most 2 arguments (3 given)",
    // not the invalid-keyword error.  When *every* arg is a keyword, CPython
    // words it "takes at most N keyword arguments" instead.
    if args.len() > max_args {
        let noun = if args.iter().all(|a| a.name.is_some()) {
            "keyword arguments"
        } else {
            "arguments"
        };
        return Err(PyError::named(
            "TypeError",
            format!(
                "{function_name}() takes at most {max_args} {noun} ({} given)",
                args.len()
            ),
        ));
    }

    let mut slots: Vec<Option<Value>> = vec![None; params.len()];

    // Assign positional args to leading slots in order.
    let mut next_pos = 0usize;
    for a in args.iter().filter(|a| a.name.is_none()) {
        // `args.len() <= max_args` already guarantees we don't overrun.
        slots[next_pos] = Some(a.value.clone());
        next_pos += 1;
    }

    // Bind keyword args by name.
    for a in args.iter().filter(|a| a.name.is_some()) {
        let name = a.name.as_ref().unwrap();
        match params.iter().position(|p| p == name) {
            Some(idx) if keyword_ok[idx] => {
                if slots[idx].is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "argument for {function_name}() given by name ('{name}') and position ({})",
                            idx + 1
                        ),
                    ));
                }
                slots[idx] = Some(a.value.clone());
            }
            // Either an unknown name, or a positional-only parameter supplied
            // by keyword — both surface as the invalid-keyword error.
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{name}' is an invalid keyword argument for {function_name}()"),
                ));
            }
        }
    }

    Ok(slots)
}

/// Returns true if the named builtin function is a classmethod on `object`.
///
/// CPython's `object.__init_subclass__` is a `classmethod_descriptor`; all
/// other `object` dunders (`__init__`, `__str__`, etc.) are instance-method
/// `wrapper_descriptor`s.  When `super()` is used inside a classmethod and
/// the MRO walk resolves to a `BuiltinFunction` sentinel, we must only bind
/// `cls` (not the class as if it were an instance `self`) for the known-
/// classmethod entries.
pub(crate) fn is_builtin_classmethod(fn_name: &str) -> bool {
    matches!(
        fn_name,
        "object.__init_subclass__"
            | "object.__subclasshook__"
            | "type.__prepare__"
            | "collections.abc.__instancecheck__"
            | "collections.abc.__subclasshook__"
            | "collections.abc.__subclasscheck__"
    )
}

pub(crate) fn py_mod_i64(a: i64, b: i64) -> i64 {
    // `i64::MIN % -1` overflows; the mathematical result is 0.
    let mut remainder = a.wrapping_rem(b);
    if (remainder > 0 && b < 0) || (remainder < 0 && b > 0) {
        remainder += b;
    }
    remainder
}

/// Port of CPython's `float_divmod` (Objects/floatobject.c).
///
/// Returns `(floordiv, mod)` for non-zero divisor `b`, using `fmod` so that
/// infinities and signed zeros propagate exactly as in CPython:
///   - `divmod(inf, 1)` → `(nan, nan)` (the `(a - mod)/b` quotient is nan)
///   - `divmod(5.0, inf)` → `(0.0, 5.0)`, `divmod(-5.0, inf)` → `(-1.0, inf)`
///
/// The remainder matches the `%` operator and the quotient matches `//`,
/// keeping `divmod(a, b) == (a // b, a % b)` for floats.  The caller is
/// responsible for raising `ZeroDivisionError` when `b == 0`.
pub(crate) fn float_divmod(a: f64, b: f64) -> (f64, f64) {
    let mut mod_ = a % b; // fmod(a, b)
    let mut div = (a - mod_) / b;
    if mod_ != 0.0 {
        // Snap the remainder's sign to the divisor's, adjusting the quotient.
        if (b < 0.0) != (mod_ < 0.0) {
            mod_ += b;
            div -= 1.0;
        }
    } else {
        // The remainder is zero; ensure it has the sign of the divisor.
        mod_ = 0.0_f64.copysign(b);
    }
    let floordiv = if div != 0.0 {
        let fl = div.floor();
        // Round-half-up on the quotient boundary, as CPython does.
        if div - fl > 0.5 {
            fl + 1.0
        } else {
            fl
        }
    } else {
        // div is zero; ensure it has the sign of a/b.
        0.0_f64.copysign(a / b)
    };
    (floordiv, mod_)
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
        // BigInt is a valid integer type but almost certainly out of range for
        // any realistic sequence length.  Try to narrow to i64; if it doesn't
        // fit, report IndexError (CPython: "cannot fit '...' into an
        // index-sized integer" when __index__ returns a large int — the
        // nearest equivalent here is IndexError for an unreachable index).
        ValueKind::BigInt(big) => match big.to_i64() {
            Some(v) => v,
            None => return Err(PyError::named("IndexError", oor_msg)),
        },
        _ => {
            // CPython uses a different message format for string vs other sequences.
            let type_name = value_type_name_str(index);
            let msg = if label == "string" {
                format!("string indices must be integers, not '{type_name}'")
            } else {
                format!("{label} indices must be integers or slices, not {type_name}")
            };
            return Err(PyError::named("TypeError", msg));
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
    let (base, extra_bases) = {
        let borrowed = class.borrow();
        (borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    if base.is_some_and(|base| class_is_subclass_of(&base, expected)) {
        return true;
    }
    extra_bases.iter().any(|b| class_is_subclass_of(b, expected))
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
pub(crate) fn class_chain_contains_name(class: &Rc<RefCell<PyClass>>, name: &str) -> bool {
    // Walk the base chain by reference — each node is a distinct `RefCell`, so
    // recursing while the current borrow is held never conflicts.  Avoids the
    // per-node `String`/`Rc`/`Vec` clones that made this hot helper costly
    // (issue #1967).
    let borrowed = class.borrow();
    if borrowed.name == name {
        return true;
    }
    if let Some(base) = &borrowed.base {
        if class_chain_contains_name(base, name) {
            return true;
        }
    }
    borrowed
        .extra_bases
        .iter()
        .any(|b| class_chain_contains_name(b, name))
}

/// Set of special-exception classifications a class may inherit, all derived
/// in a single non-cloning MRO walk (issue #1967).  Previously
/// `instantiate_exception` ran ~12 separate cloning base-chain scans per
/// constructed exception; this collects every match in one pass.
///
/// The classification result is identical to running
/// [`class_chain_contains_name`] once per name: a flag is `true` iff the
/// corresponding name appears anywhere in the class's base chain (so user
/// subclasses of `OSError`, `StopIteration`, … inherit the special handling,
/// preserving the behaviour from issue #612).
#[derive(Default)]
pub(crate) struct ExcClassKinds {
    pub(crate) stop_iteration: bool,
    pub(crate) syntax_error: bool,
    pub(crate) os_error: bool,
    pub(crate) system_exit: bool,
    pub(crate) unicode_decode_error: bool,
    pub(crate) unicode_encode_error: bool,
    pub(crate) unicode_translate_error: bool,
    pub(crate) name_error: bool,
    pub(crate) import_error: bool,
    pub(crate) attribute_error: bool,
    pub(crate) base_exception_group: bool,
    /// `true` if any class in the MRO defines a user (Python) `__new__`.
    /// Plain built-in exceptions and their attribute-only subclasses leave this
    /// `false`, letting `construct_exception_instance` skip the `__new__` MRO
    /// lookup entirely on the hot `raise ValueError("x")` path.  A built-in
    /// `__new__` can never shadow a user `__new__` (built-in `__new__` only
    /// lives on base classes, which are less derived), so a node-wise "any user
    /// `__new__`" test matches the prior MRO-first `.filter(UserFunction)` check.
    pub(crate) has_user_new: bool,
    /// `true` if any class in the MRO defines a user (Python) `__init__`.
    /// Same rationale as [`has_user_new`](Self::has_user_new): `BaseException`
    /// supplies a built-in `__init__`, so plain built-in exceptions stay `false`.
    pub(crate) has_user_init: bool,
}

impl ExcClassKinds {
    fn merge_name(&mut self, name: &str) {
        match name {
            "StopIteration" => self.stop_iteration = true,
            "SyntaxError" => self.syntax_error = true,
            "OSError" => self.os_error = true,
            "SystemExit" => self.system_exit = true,
            "UnicodeDecodeError" => self.unicode_decode_error = true,
            "UnicodeEncodeError" => self.unicode_encode_error = true,
            "UnicodeTranslateError" => self.unicode_translate_error = true,
            "NameError" => self.name_error = true,
            "ImportError" => self.import_error = true,
            "AttributeError" => self.attribute_error = true,
            "BaseExceptionGroup" => self.base_exception_group = true,
            _ => {}
        }
    }
}

/// Classify `class` against every special built-in exception name in a single
/// borrowing walk of its base chain (issue #1967).
pub(crate) fn classify_exception_class(class: &Rc<RefCell<PyClass>>) -> ExcClassKinds {
    let mut kinds = ExcClassKinds::default();
    fn walk(class: &Rc<RefCell<PyClass>>, kinds: &mut ExcClassKinds) {
        let borrowed = class.borrow();
        kinds.merge_name(&borrowed.name);
        // Detect user-defined __new__/__init__ in the same walk so the caller
        // can skip the dedicated MRO lookups for plain built-in exceptions.
        if !kinds.has_user_new
            && matches!(
                borrowed.attrs.get("__new__").map(Value::kind),
                Some(ValueKind::UserFunction(_))
            )
        {
            kinds.has_user_new = true;
        }
        if !kinds.has_user_init
            && matches!(
                borrowed.attrs.get("__init__").map(Value::kind),
                Some(ValueKind::UserFunction(_))
            )
        {
            kinds.has_user_init = true;
        }
        if let Some(base) = &borrowed.base {
            walk(base, kinds);
        }
        for b in &borrowed.extra_bases {
            walk(b, kinds);
        }
    }
    walk(class, &mut kinds);
    kinds
}

/// Return `true` if any class in the MRO of `class` (including `class`
/// itself, excluding the implicit `object` root) has `slots: None`.
///
/// CPython rule: `__slots__` only prevents instance `__dict__` creation when
/// *every* class in the MRO (between the leaf class and `object`) declares
/// `__slots__`.  If any ancestor has no `__slots__`, it contributes a
/// `__dict__` and slot enforcement is bypassed.  This covers both:
/// - `class Child(SlottedParent): pass` — Child has `slots: None`
/// - `class GrandChild(Child): __slots__ = ('x',)` — Child has `slots: None`
pub(crate) fn mro_has_unslotted_ancestor(class: &Rc<RefCell<PyClass>>) -> bool {
    // Stop at `object` (no explicit base = treated as object).
    let (slots, base, extra_bases) = {
        let borrowed = class.borrow();
        (borrowed.slots.clone(), borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    if slots.is_none() {
        return true;
    }
    if let Some(ref b) = base {
        if !Rc::ptr_eq(b, &object_class_singleton()) && mro_has_unslotted_ancestor(b) {
            return true;
        }
    }
    extra_bases
        .iter()
        .filter(|b| !Rc::ptr_eq(b, &object_class_singleton()))
        .any(mro_has_unslotted_ancestor)
}

/// Return `true` if `name` is listed in the `__slots__` of `class` or any of
/// its ancestors (excluding the implicit `object` root).
///
/// CPython allocates a slot descriptor for every name in `__slots__` along the
/// MRO, so the set of allowed slot names on an instance is the *union* of all
/// `__slots__` across the chain — not just the leaf class's.  This mirrors the
/// traversal of `mro_has_unslotted_ancestor`.
pub(crate) fn mro_slot_allows(class: &Rc<RefCell<PyClass>>, name: &str) -> bool {
    let (slots, base, extra_bases) = {
        let borrowed = class.borrow();
        (borrowed.slots.clone(), borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    if let Some(ref slot_set) = slots {
        if slot_set.contains(name) {
            return true;
        }
    }
    if let Some(ref b) = base {
        if !Rc::ptr_eq(b, &object_class_singleton()) && mro_slot_allows(b, name) {
            return true;
        }
    }
    extra_bases
        .iter()
        .filter(|b| !Rc::ptr_eq(b, &object_class_singleton()))
        .any(|b| mro_slot_allows(b, name))
}

/// Return `true` if instances of `class` must NOT expose a `__dict__`
/// (issue #2076).  CPython suppresses the instance `__dict__` when the class
/// declares `__slots__`, none of the slots is `'__dict__'`, and no ancestor in
/// the MRO is unslotted (an unslotted ancestor reintroduces `tp_dictoffset`).
/// Mirrors the condition guarding `__slots__` setattr enforcement.
pub(crate) fn class_suppresses_instance_dict(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().slots.is_some()
        && !mro_slot_allows(class, "__dict__")
        && !mro_has_unslotted_ancestor(class)
}

/// Handle `instance.__dict__ = value` (issue #1942).
///
/// CPython's `tp_setattro` routes assignment to the `__dict__` slot through a
/// dedicated setter that *replaces* the instance dict wholesale (rather than
/// storing an attribute literally named `__dict__`).  The value must be a
/// `dict`; anything else raises `TypeError`.
///
/// pyrust's instance attrs map is `IndexMap<String, Value>`, so only string
/// keys are representable as attributes.  CPython accepts a dict with non-str
/// keys here (they're simply never accessible as attributes); we mirror the
/// observable attribute behaviour by keeping only the string-keyed entries.
///
/// `other.__dict__` evaluates to an `instance_dict` proxy in pyrust (CPython
/// returns the backing dict itself), so we also accept a proxy here and copy
/// its visible entries.  Live aliasing (`w.__dict__ is other.__dict__`) is not
/// reproduced — that needs first-class dict-backed instance storage (#1942
/// follow-up).
pub(crate) fn replace_instance_dict(
    instance: &Rc<RefCell<PyInstance>>,
    value: &Value,
) -> Result<()> {
    let entries = match value.as_dict() {
        Some(map) => map
            .iter()
            .filter_map(|(k, v)| match k {
                PyKey::Str(s) => s.as_str().map(|s| (s.to_string(), v.clone())),
                _ => None,
            })
            .collect::<Vec<(String, Value)>>(),
        None => match pyrust_builtins::instance_dict::as_instance_dict_items(value) {
            Some(items) => items
                .into_iter()
                .filter_map(|(k, v)| match k {
                    PyKey::Str(s) => s.as_str().map(|s| (s.to_string(), v)),
                    _ => None,
                })
                .collect::<Vec<(String, Value)>>(),
            None => {
                let type_name = pyrust_core::builtin_type_name(value);
                return Err(pyrust_core::type_err!(
                    "__dict__ must be set to a dictionary, not a '{type_name}'"
                ));
            }
        },
    };
    let mut borrow = instance.borrow_mut();
    borrow.attrs.clear();
    for (k, v) in entries {
        borrow.attrs.insert(k, v);
    }
    Ok(())
}

/// Return the errno-specific OSError subclass `Rc` for a given errno value,
/// mirroring CPython 3.12's `_Py_errnomap` table in `Objects/exceptions.c`.
/// Returns `None` when the errno has no mapped subclass (plain `OSError` is
/// used in that case).  Only called when the constructor class is exactly
/// `OSError`; subclasses are never remapped.
fn oserror_subclass_for_errno(errno: i64) -> Option<Rc<RefCell<PyClass>>> {
    // CPython's _Py_errnomap (Linux errno values):
    //   1  EPERM        → PermissionError
    //   2  ENOENT       → FileNotFoundError
    //   3  ESRCH        → ProcessLookupError
    //   4  EINTR        → InterruptedError
    //  10  ECHILD       → ChildProcessError
    //  11  EAGAIN       → BlockingIOError
    //  13  EACCES       → PermissionError
    //  17  EEXIST       → FileExistsError
    //  20  ENOTDIR      → NotADirectoryError
    //  21  EISDIR       → IsADirectoryError
    //  32  EPIPE        → BrokenPipeError
    // 103  ECONNABORTED → ConnectionAbortedError
    // 104  ECONNRESET   → ConnectionResetError
    // 108  ESHUTDOWN    → BrokenPipeError
    // 110  ETIMEDOUT    → TimeoutError
    // 111  ECONNREFUSED → ConnectionRefusedError
    // 114  EALREADY     → BlockingIOError
    // 115  EINPROGRESS  → BlockingIOError
    let subclass_name = match errno {
        1 | 13 => "PermissionError",
        2 => "FileNotFoundError",
        3 => "ProcessLookupError",
        4 => "InterruptedError",
        10 => "ChildProcessError",
        11 | 114 | 115 => "BlockingIOError",
        17 => "FileExistsError",
        20 => "NotADirectoryError",
        21 => "IsADirectoryError",
        32 | 108 => "BrokenPipeError",
        103 => "ConnectionAbortedError",
        104 => "ConnectionResetError",
        110 => "TimeoutError",
        111 => "ConnectionRefusedError",
        _ => return None,
    };
    EXC_CLASS_CACHE.with(|cache| {
        cache
            .iter()
            .find(|(name, _)| *name == subclass_name)
            .map(|(_, cls)| Rc::clone(cls))
    })
}

pub(crate) fn instantiate_exception(class: Rc<RefCell<PyClass>>, args: Vec<Value>) -> Value {
    // Classify the class against every special built-in exception name in a
    // single non-cloning MRO walk (issue #1967), instead of running ~12
    // separate cloning base-chain scans per constructed exception.  The result
    // is identical to the previous per-name walks: each flag is true iff that
    // name appears in the class's base chain, so user subclasses (e.g.
    // `class MyStop(StopIteration)`) still inherit the special handling (#612).
    let kinds = classify_exception_class(&class);
    instantiate_exception_with_kinds(class, args, &kinds)
}

/// Like [`instantiate_exception`] but takes a pre-computed [`ExcClassKinds`].
/// `construct_exception_instance` already classifies the class once to handle
/// keyword args / argument validation; threading that result through here
/// avoids a second redundant MRO classification walk per constructed exception.
pub(crate) fn instantiate_exception_with_kinds(
    class: Rc<RefCell<PyClass>>,
    args: Vec<Value>,
    kinds: &ExcClassKinds,
) -> Value {
    // The common case (a plain built-in exception) sets exactly two attributes
    // — `args` and `__traceback__` — so reserve for them up front to avoid the
    // Vec growth realloc that would otherwise happen on the second insert.
    let mut attrs = InstanceAttrs::with_capacity(2);
    // CPython 3.12: StopIteration.__init__ sets self.value = args[0] if args else None.
    let is_stop_iteration = kinds.stop_iteration;
    let is_syntax_error = kinds.syntax_error;
    // OSError is the canonical name; IOError and EnvironmentError are aliases that
    // share the same Rc, so checking for "OSError" in the chain suffices.
    let is_os_error = kinds.os_error;
    let is_system_exit = kinds.system_exit;
    // Decode wins over encode wins over translate, matching the original
    // short-circuiting precedence.
    let is_unicode_decode_error = kinds.unicode_decode_error;
    let is_unicode_encode_error = !is_unicode_decode_error && kinds.unicode_encode_error;
    let is_unicode_translate_error =
        !is_unicode_decode_error && !is_unicode_encode_error && kinds.unicode_translate_error;
    // CPython 3.12: NameError (and its subclass UnboundLocalError) have a `.name`
    // attribute.  User-constructed instances (`NameError('msg')`) have `name = None`.
    // Interpreter-raised instances set the name via `instantiate_name_error` instead.
    let is_name_error = kinds.name_error;
    // CPython 3.12: ImportError (and its subclass ModuleNotFoundError) have `.name`
    // and `.path` attributes.  User-constructed instances (`ImportError('msg')`) have
    // both set to `None`.  Interpreter-raised instances set them via
    // `instantiate_import_error` instead.
    let is_import_error = kinds.import_error;
    // CPython 3.12: AttributeError has `.name` and `.obj` attributes.
    // User-constructed instances (`AttributeError('msg')`) have both set to `None`.
    // Interpreter-raised instances set them via `instantiate_attribute_error` instead.
    let is_attribute_error = kinds.attribute_error;
    // PEP 654 (Python 3.11+): BaseExceptionGroup / ExceptionGroup.
    // Both have `.message` (str) and `.exceptions` (tuple of exceptions).
    let is_base_exception_group = kinds.base_exception_group;
    // Pass `&str` keys (not `String`): `insert` interns the key into a shared
    // `Rc<str>`, so a temporary `String` per key per raise would be allocated
    // only to be dropped immediately.
    attrs.insert("args", Value::tuple(args.clone()));
    // CPython 3.12: every BaseException instance has __traceback__ initialised
    // to None at __new__ time.  The VM's handle_vm_error overwrites it with a
    // real traceback object once the exception propagates through a frame.
    attrs.insert("__traceback__", Value::none());
    if is_stop_iteration {
        let val = args.first().cloned().unwrap_or_else(Value::none);
        attrs.insert("value".to_string(), val);
    } else if is_system_exit {
        // CPython 3.12 SystemExit.__init__: code = args[0] if 1 arg, tuple(args) if
        // multiple args, None if no args.  For the multi-arg case CPython sets
        // self.code = self.args (the same object), so clone the already-inserted
        // args tuple to share the same obj_id and preserve `e.code is e.args`.
        let code = match args.len() {
            0 => Value::none(),
            1 => args[0].clone(),
            _ => attrs.get("args").cloned().unwrap_or_else(Value::none),
        };
        attrs.insert("code".to_string(), code);
    } else if is_syntax_error {
        // CPython 3.12 SyntaxError.__init__: always initialise all structured
        // attributes.  With 1 arg: msg = args[0], rest = None.  With 2 args
        // where args[1] is a sequence of at least 4 elements, unpack:
        //   (filename, lineno, offset, text[, end_lineno, end_offset])
        // CPython accepts any sequence (tuple OR list) for args[1]; it iterates
        // and unpacks it.  Callers are responsible for raising TypeError when
        // args[1] is a non-sequence or has the wrong number of elements
        // (call_class_expanded validates before reaching here).
        let msg = args.first().cloned().unwrap_or_else(Value::none);
        let mut filename = Value::none();
        let mut lineno = Value::none();
        let mut offset = Value::none();
        let mut text = Value::none();
        let mut end_lineno = Value::none();
        let mut end_offset = Value::none();
        if args.len() >= 2 {
            // Accept both tuple and list — CPython's SyntaxError.__init__ iterates
            // the second argument regardless of its concrete type.
            let items_opt: Option<Vec<Value>> = args[1]
                .as_tuple()
                .map(|s| s.to_vec())
                .or_else(|| args[1].as_list().map(|s| s.to_vec()));
            if let Some(items) = items_opt {
                if items.len() >= 4 {
                    filename = items[0].clone();
                    lineno = items[1].clone();
                    offset = items[2].clone();
                    text = items[3].clone();
                }
                if items.len() >= 6 {
                    end_lineno = items[4].clone();
                    end_offset = items[5].clone();
                }
            }
        }
        attrs.insert("msg".to_string(), msg);
        attrs.insert("filename".to_string(), filename);
        attrs.insert("lineno".to_string(), lineno);
        attrs.insert("offset".to_string(), offset);
        attrs.insert("text".to_string(), text);
        attrs.insert("end_lineno".to_string(), end_lineno);
        attrs.insert("end_offset".to_string(), end_offset);
        // CPython 3.12 also initialises print_file_and_line (always None for
        // user-constructed instances; only set by the C compile-phase injector).
        attrs.insert("print_file_and_line".to_string(), Value::none());
    } else if is_os_error {
        // CPython 3.12 OSError.__init__: populate errno/strerror/filename/filename2.
        // With 0 or 1 args: all None.  With 2 args: errno=args[0], strerror=args[1].
        // With 3 args: additionally filename=args[2].
        // With 5 args: args[3]=winerror (ignored on non-Windows), args[4]=filename2.
        if args.len() >= 2 {
            attrs.insert("errno".to_string(), args[0].clone());
            attrs.insert("strerror".to_string(), args[1].clone());
            // CPython 3.12: OSError.__init__ always sets self.args = (errno, strerror)
            // regardless of how many positional arguments were supplied.  The filename
            // (and filename2) are stored as dedicated instance attributes, not in args.
            attrs.insert(
                "args".to_string(),
                Value::tuple(vec![args[0].clone(), args[1].clone()]),
            );
        } else {
            attrs.insert("errno".to_string(), Value::none());
            attrs.insert("strerror".to_string(), Value::none());
        }
        attrs.insert(
            "filename".to_string(),
            args.get(2).cloned().unwrap_or_else(Value::none),
        );
        // filename2 is set by the 5-arg form: OSError(errno, strerror, fname, winerror, fname2)
        attrs.insert(
            "filename2".to_string(),
            args.get(4).cloned().unwrap_or_else(Value::none),
        );
        // CPython 3.12 OSError.__new__ remaps to an errno-specific subclass when
        // called as exactly OSError(errno, strerror[, ...]) where there are at
        // least 2 args and the first is an integer.  Single-arg calls (e.g.
        // OSError(2)) are NOT remapped.  Subclasses (FileNotFoundError, …) are
        // also not remapped — only the plain OSError call triggers the lookup.
        if args.len() >= 2 && class.borrow().name == "OSError" {
            if let Some(errno_int) = args[0].as_int() {
                if let Some(subclass) = oserror_subclass_for_errno(errno_int) {
                    return Value::py_instance(Rc::new(RefCell::new(PyInstance {
                        class: subclass,
                        attrs,
                    })));
                }
            }
        }
    } else if is_unicode_decode_error || is_unicode_encode_error || is_unicode_translate_error {
        // CPython 3.12: UnicodeDecodeError(encoding, object, start, end, reason)
        //               UnicodeEncodeError(encoding, object, start, end, reason)
        //               UnicodeTranslateError(object, start, end, reason)
        // Arg count validation is done in call_class_expanded before reaching here.
        // We set attributes from args when the right number are present.
        unicode_exc_set_attrs(&mut attrs, &args, is_unicode_decode_error || is_unicode_encode_error);
    }
    if is_name_error {
        // CPython 3.12: user-constructed NameError (and UnboundLocalError) instances
        // always have a `.name` attribute, defaulting to `None`.  Interpreter-raised
        // instances set the name via `instantiate_name_error` with the actual identifier.
        attrs.insert("name".to_string(), Value::none());
    }
    if is_import_error {
        // CPython 3.12: user-constructed ImportError (and ModuleNotFoundError) instances
        // always have `.name` and `.path` attributes, both defaulting to `None`.
        // Interpreter-raised instances set them via `instantiate_import_error`.
        attrs.insert("name".to_string(), Value::none());
        attrs.insert("path".to_string(), Value::none());
    }
    if is_attribute_error {
        // CPython 3.12: user-constructed AttributeError instances always have `.name`
        // and `.obj` attributes, both defaulting to `None`.  Interpreter-raised
        // instances set them via `instantiate_attribute_error` with the actual values.
        attrs.insert("name".to_string(), Value::none());
        attrs.insert("obj".to_string(), Value::none());
    }
    if is_base_exception_group {
        // PEP 654: BaseExceptionGroup(message, exceptions).
        // Set `.message` = args[0] (str) and `.exceptions` = args[1] as a tuple.
        // args[0] defaults to "" and args[1] defaults to an empty tuple on bad input,
        // but CPython validates in __new__; we set what we have.
        let message = args.first().cloned().unwrap_or_else(|| Value::string(String::new()));
        let exceptions_raw = args.get(1).cloned().unwrap_or_else(|| Value::tuple(vec![]));
        // Normalise the exceptions to a tuple (accept list too).
        let exceptions = if exceptions_raw.as_tuple().is_some() {
            exceptions_raw
        } else if let Some(lst) = exceptions_raw.as_list() {
            Value::tuple(lst.to_vec())
        } else {
            Value::tuple(vec![])
        };
        attrs.insert("message".to_string(), message);
        attrs.insert("exceptions".to_string(), exceptions);
    }
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate an `OSError` (or subclass) with the structured attributes that
/// CPython 3.12 sets when an OS error is raised from a real OS operation:
/// `errno`, `strerror`, `filename` (and `filename2 = None`).
///
/// The `args` tuple is set to `(errno, strerror)` to match CPython 3.12
/// behaviour (the 2-arg form).  The `class` must already be the correct
/// subclass (`FileNotFoundError`, `PermissionError`, etc.).
pub(crate) fn instantiate_os_error(
    class: Rc<RefCell<PyClass>>,
    errno: i64,
    strerror: String,
    filename: Option<String>,
    filename2: Option<String>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    let errno_val = Value::int(errno);
    let strerror_val = Value::string(strerror);
    attrs.insert(
        "args".to_string(),
        Value::tuple(vec![errno_val.clone(), strerror_val.clone()]),
    );
    attrs.insert("errno".to_string(), errno_val);
    attrs.insert("strerror".to_string(), strerror_val);
    attrs.insert(
        "filename".to_string(),
        filename.map(Value::string).unwrap_or_else(Value::none),
    );
    attrs.insert(
        "filename2".to_string(),
        filename2.map(Value::string).unwrap_or_else(Value::none),
    );
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
    let mut attrs = InstanceAttrs::new();
    attrs.insert("args".to_string(), Value::tuple(vec![Value::string(message)]));
    let name_val = match module_name {
        Some(n) => Value::string(n),
        None => Value::none(),
    };
    attrs.insert("name".to_string(), name_val);
    attrs.insert("path".to_string(), Value::none());
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate a `NameError` or `UnboundLocalError` with the `.name` instance
/// attribute set, matching CPython 3.12 parity.
///
/// CPython 3.12: when the interpreter raises `NameError` for a missing
/// identifier, it stores the identifier string as `self.name`.  User-
/// constructed instances (e.g. `NameError('msg')`) have `name = None`.
/// `UnboundLocalError.name` is always `None` in CPython 3.12.
///
/// `name` is stored as `.name`; pass `None` for `UnboundLocalError` or when
/// the identifier is not available.
pub(crate) fn instantiate_name_error(
    class: Rc<RefCell<PyClass>>,
    message: String,
    name: Option<String>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("args".to_string(), Value::tuple(vec![Value::string(message)]));
    attrs.insert("__traceback__".to_string(), Value::none());
    let name_val = match name {
        Some(n) => Value::string(n),
        None => Value::none(),
    };
    attrs.insert("name".to_string(), name_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate an `AttributeError` with the `.name` and `.obj` instance
/// attributes set, matching CPython 3.12 parity.
///
/// CPython 3.12: when the interpreter raises `AttributeError` for a missing
/// attribute, it stores the attribute name as `self.name` and the receiver
/// object as `self.obj`.  User-constructed instances (e.g.
/// `AttributeError('msg')`) have both set to `None`.
///
/// `name` is stored as `.name`; pass `None` when not available.
/// `obj` is stored as `.obj`; pass `None` when not available.
pub(crate) fn instantiate_attribute_error(
    class: Rc<RefCell<PyClass>>,
    message: String,
    name: Option<String>,
    obj: Option<Value>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("args".to_string(), Value::tuple(vec![Value::string(message)]));
    attrs.insert("__traceback__".to_string(), Value::none());
    attrs.insert(
        "name".to_string(),
        name.map(Value::string).unwrap_or_else(Value::none),
    );
    attrs.insert("obj".to_string(), obj.unwrap_or_else(Value::none));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Set the five Unicode-exception structured attributes (`encoding`, `object`,
/// `start`, `end`, `reason`) on an already-allocated `attrs` map from a
/// positional argument list.
///
/// Used by both `instantiate_exception` (for user-constructed calls) and
/// `base_exception_init` (for `super().__init__(...)` in subclasses).
///
/// `has_encoding` is `true` for `UnicodeDecodeError`/`UnicodeEncodeError`
/// (which take 5 args: encoding, object, start, end, reason) and `false`
/// for `UnicodeTranslateError` (4 args: object, start, end, reason).
///
/// If the arg count doesn't match the expected signature, this function is a
/// no-op — arg count validation is the caller's responsibility.
pub(crate) fn unicode_exc_set_attrs(
    attrs: &mut InstanceAttrs,
    args: &[Value],
    has_encoding: bool,
) {
    if has_encoding {
        if args.len() != 5 {
            return;
        }
        attrs.insert("encoding".to_string(), args[0].clone());
        attrs.insert("object".to_string(), args[1].clone());
        attrs.insert("start".to_string(), args[2].clone());
        attrs.insert("end".to_string(), args[3].clone());
        attrs.insert("reason".to_string(), args[4].clone());
    } else {
        if args.len() != 4 {
            return;
        }
        attrs.insert("object".to_string(), args[0].clone());
        attrs.insert("start".to_string(), args[1].clone());
        attrs.insert("end".to_string(), args[2].clone());
        attrs.insert("reason".to_string(), args[3].clone());
    }
}

/// Instantiate a `UnicodeDecodeError` with its five structured attributes set
/// from the raw Rust data produced by an internal decoding operation (e.g.
/// `bytes.decode()`).  Used by the VM when materialising a
/// `PyError::UnicodeDecodeError` variant.
pub(crate) fn instantiate_unicode_decode_error(
    class: Rc<RefCell<PyClass>>,
    encoding: String,
    object: Vec<u8>,
    start: usize,
    end: usize,
    reason: String,
) -> Value {
    let enc_val = Value::string(&encoding);
    let obj_val = Value::bytes(object);
    let start_val = Value::int(start as i64);
    let end_val = Value::int(end as i64);
    let reason_val = Value::string(&reason);
    let mut attrs = InstanceAttrs::new();
    attrs.insert(
        "args".to_string(),
        Value::tuple(vec![
            enc_val.clone(),
            obj_val.clone(),
            start_val.clone(),
            end_val.clone(),
            reason_val.clone(),
        ]),
    );
    attrs.insert("__traceback__".to_string(), Value::none());
    attrs.insert("encoding".to_string(), enc_val);
    attrs.insert("object".to_string(), obj_val);
    attrs.insert("start".to_string(), start_val);
    attrs.insert("end".to_string(), end_val);
    attrs.insert("reason".to_string(), reason_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate a `UnicodeEncodeError` with its five structured attributes set
/// from the raw Rust data produced by an internal encoding operation (e.g.
/// `str.encode()`).  Used by the VM when materialising a
/// `PyError::UnicodeEncodeError` variant.
pub(crate) fn instantiate_unicode_encode_error(
    class: Rc<RefCell<PyClass>>,
    encoding: String,
    object: String,
    start: usize,
    end: usize,
    reason: String,
) -> Value {
    let enc_val = Value::string(&encoding);
    let obj_val = Value::string(&object);
    let start_val = Value::int(start as i64);
    let end_val = Value::int(end as i64);
    let reason_val = Value::string(&reason);
    let mut attrs = InstanceAttrs::new();
    attrs.insert(
        "args".to_string(),
        Value::tuple(vec![
            enc_val.clone(),
            obj_val.clone(),
            start_val.clone(),
            end_val.clone(),
            reason_val.clone(),
        ]),
    );
    attrs.insert("__traceback__".to_string(), Value::none());
    attrs.insert("encoding".to_string(), enc_val);
    attrs.insert("object".to_string(), obj_val);
    attrs.insert("start".to_string(), start_val);
    attrs.insert("end".to_string(), end_val);
    attrs.insert("reason".to_string(), reason_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
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
    //       AssertionError, AttributeError, EOFError, StopIteration, SyntaxError
    //         → IndentationError → TabError
    //       MemoryError, ImportError → ModuleNotFoundError
    //       OSError → BlockingIOError, ChildProcessError, FileExistsError,
    //                 FileNotFoundError, InterruptedError, IsADirectoryError,
    //                 NotADirectoryError, PermissionError, ProcessLookupError,
    //                 TimeoutError, io.UnsupportedOperation
    //                 ConnectionError → BrokenPipeError, ConnectionAbortedError,
    //                                   ConnectionRefusedError, ConnectionResetError
    //       Warning → UserWarning, DeprecationWarning, PendingDeprecationWarning,
    //                 RuntimeWarning, SyntaxWarning, ResourceWarning, FutureWarning,
    //                 ImportWarning, UnicodeWarning, BytesWarning, EncodingWarning
    //     SystemExit, GeneratorExit, KeyboardInterrupt (direct BaseException children)
    let mk = |name: &str, base: Option<Rc<RefCell<PyClass>>>| {
        let class = Rc::new(RefCell::new(PyClass::new(
            name,
            name,
            base.clone(),
            IndexMap::new(),
        )));
        if let Some(b) = base {
            b.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        }
        class
    };
    let base_exception = mk("BaseException", None);
    // Install `add_note` (Python 3.11+ — issue #1067) on BaseException so that
    // every exception subclass inherits it via `lookup_class_attr`.
    {
        static ADD_NOTE_NAME: std::sync::LazyLock<&'static str> =
            std::sync::LazyLock::new(|| {
                Box::leak("BaseException.add_note".to_string().into_boxed_str())
            });
        base_exception.borrow_mut().attrs.insert(
            "add_note".to_string(),
            Value::builtin_function(*ADD_NOTE_NAME),
        );
    }
    // Issue #1112: install `BaseException.__init__` so that `super().__init__(…)`
    // in a user-defined exception subclass resolves via MRO lookup and updates
    // `.args` (and `.value` for StopIteration) on the already-constructed instance.
    base_exception
        .borrow_mut()
        .attrs
        .insert("__init__".to_string(), Value::builtin_function("BaseException.__init__"));
    // Issue #1441: install `with_traceback` on BaseException so every exception
    // subclass inherits it.  Sets __traceback__ and returns self.
    base_exception.borrow_mut().attrs.insert(
        "with_traceback".to_string(),
        Value::builtin_function("BaseException.with_traceback"),
    );
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
    let blocking_io_error = mk("BlockingIOError", Some(Rc::clone(&os_error)));
    let child_process_error = mk("ChildProcessError", Some(Rc::clone(&os_error)));
    let interrupted_error = mk("InterruptedError", Some(Rc::clone(&os_error)));
    let is_a_directory_error = mk("IsADirectoryError", Some(Rc::clone(&os_error)));
    let not_a_directory_error = mk("NotADirectoryError", Some(Rc::clone(&os_error)));
    let permission_error = mk("PermissionError", Some(Rc::clone(&os_error)));
    let process_lookup_error = mk("ProcessLookupError", Some(Rc::clone(&os_error)));
    let timeout_error = mk("TimeoutError", Some(Rc::clone(&os_error)));
    let connection_error = mk("ConnectionError", Some(Rc::clone(&os_error)));
    let broken_pipe_error = mk("BrokenPipeError", Some(Rc::clone(&connection_error)));
    let connection_aborted_error = mk("ConnectionAbortedError", Some(Rc::clone(&connection_error)));
    let connection_refused_error = mk("ConnectionRefusedError", Some(Rc::clone(&connection_error)));
    let connection_reset_error = mk("ConnectionResetError", Some(Rc::clone(&connection_error)));
    // CPython: io.UnsupportedOperation inherits from both OSError and ValueError
    // (multiple inheritance).  pyrust uses single-inheritance; we pick OSError
    // as the primary base since that is the first in CPython's MRO and what most
    // user code catches (`except OSError`).  The class is registered under both
    // "io.UnsupportedOperation" (the dotted name used by raise sites) and
    // "UnsupportedOperation" (the bare name printed in tracebacks).
    let unsupported_operation = mk("UnsupportedOperation", Some(Rc::clone(&os_error)));
    // Python 3.3+: IOError and EnvironmentError are aliases for OSError.
    let io_error = Rc::clone(&os_error);
    let environment_error = Rc::clone(&os_error);
    let indentation_error = mk("IndentationError", Some(Rc::clone(&syntax_error)));
    let tab_error = mk("TabError", Some(Rc::clone(&indentation_error)));
    let warning = mk("Warning", Some(Rc::clone(&exception)));
    let user_warning = mk("UserWarning", Some(Rc::clone(&warning)));
    let deprecation_warning = mk("DeprecationWarning", Some(Rc::clone(&warning)));
    let pending_deprecation_warning = mk("PendingDeprecationWarning", Some(Rc::clone(&warning)));
    let runtime_warning = mk("RuntimeWarning", Some(Rc::clone(&warning)));
    let syntax_warning = mk("SyntaxWarning", Some(Rc::clone(&warning)));
    let resource_warning = mk("ResourceWarning", Some(Rc::clone(&warning)));
    let future_warning = mk("FutureWarning", Some(Rc::clone(&warning)));
    let import_warning = mk("ImportWarning", Some(Rc::clone(&warning)));
    let unicode_warning = mk("UnicodeWarning", Some(Rc::clone(&warning)));
    let bytes_warning = mk("BytesWarning", Some(Rc::clone(&warning)));
    let encoding_warning = mk("EncodingWarning", Some(Rc::clone(&warning)));
    let unicode_encode_error = mk("UnicodeEncodeError", Some(Rc::clone(&unicode_error)));
    let unicode_decode_error = mk("UnicodeDecodeError", Some(Rc::clone(&unicode_error)));
    let unicode_translate_error = mk("UnicodeTranslateError", Some(Rc::clone(&unicode_error)));
    let buffer_error = mk("BufferError", Some(Rc::clone(&exception)));
    let reference_error = mk("ReferenceError", Some(Rc::clone(&exception)));
    let system_error = mk("SystemError", Some(Rc::clone(&exception)));
    let stop_async_iteration = mk("StopAsyncIteration", Some(Rc::clone(&exception)));
    let eof_error = mk("EOFError", Some(Rc::clone(&exception)));
    let system_exit = mk("SystemExit", Some(Rc::clone(&base_exception)));
    let generator_exit = mk("GeneratorExit", Some(Rc::clone(&base_exception)));
    let keyboard_interrupt = mk("KeyboardInterrupt", Some(Rc::clone(&base_exception)));
    // PEP 654 (Python 3.11+): BaseExceptionGroup and ExceptionGroup.
    // BaseExceptionGroup(message, exceptions) — accepts any BaseException subclass.
    // ExceptionGroup(message, exceptions)    — only accepts Exception subclasses;
    //   inherits from both BaseExceptionGroup (primary) and Exception (extra base).
    let base_exception_group = mk("BaseExceptionGroup", Some(Rc::clone(&base_exception)));
    // PEP 654: install `derive`, `subgroup`, and `split` on BaseExceptionGroup
    // so every group subclass inherits them.  These are intercepted in
    // `call_function_expanded` (they need interpreter access to call user
    // predicates / a subclass's overridden `derive`).
    {
        let mut beg = base_exception_group.borrow_mut();
        beg.attrs.insert(
            "derive".to_string(),
            Value::builtin_function("BaseExceptionGroup.derive"),
        );
        beg.attrs.insert(
            "subgroup".to_string(),
            Value::builtin_function("BaseExceptionGroup.subgroup"),
        );
        beg.attrs.insert(
            "split".to_string(),
            Value::builtin_function("BaseExceptionGroup.split"),
        );
    }
    // ExceptionGroup uses multiple inheritance: primary base = BaseExceptionGroup,
    // extra base = Exception.  Build it manually so we can set extra_bases.
    let exception_group = Rc::new(RefCell::new(PyClass {
        extra_bases: vec![Rc::clone(&exception)],
        ..PyClass::new(
            "ExceptionGroup",
            "ExceptionGroup",
            Some(Rc::clone(&base_exception_group)),
            IndexMap::new(),
        )
    }));
    base_exception_group
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&exception_group));
    exception
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&exception_group));
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
        ("EOFError", eof_error),
        ("StopIteration", stop_iteration),
        ("AttributeError", attribute_error),
        ("SyntaxError", syntax_error),
        ("IndentationError", indentation_error),
        ("TabError", tab_error),
        ("MemoryError", memory_error),
        ("ImportError", import_error),
        ("ModuleNotFoundError", module_not_found_error),
        ("UnicodeError", unicode_error),
        ("UnicodeEncodeError", unicode_encode_error),
        ("UnicodeDecodeError", unicode_decode_error),
        ("UnicodeTranslateError", unicode_translate_error),
        ("BufferError", buffer_error),
        ("ReferenceError", reference_error),
        ("SystemError", system_error),
        ("StopAsyncIteration", stop_async_iteration),
        ("OSError", os_error),
        ("IOError", io_error),
        ("EnvironmentError", environment_error),
        ("FileNotFoundError", file_not_found_error),
        ("FileExistsError", file_exists_error),
        ("BlockingIOError", blocking_io_error),
        ("ChildProcessError", child_process_error),
        ("InterruptedError", interrupted_error),
        ("IsADirectoryError", is_a_directory_error),
        ("NotADirectoryError", not_a_directory_error),
        ("PermissionError", permission_error),
        ("ProcessLookupError", process_lookup_error),
        ("TimeoutError", timeout_error),
        ("ConnectionError", connection_error),
        ("BrokenPipeError", broken_pipe_error),
        ("ConnectionAbortedError", connection_aborted_error),
        ("ConnectionRefusedError", connection_refused_error),
        ("ConnectionResetError", connection_reset_error),
        ("io.UnsupportedOperation", Rc::clone(&unsupported_operation)),
        ("UnsupportedOperation", unsupported_operation),
        ("Warning", warning),
        ("UserWarning", user_warning),
        ("DeprecationWarning", deprecation_warning),
        ("PendingDeprecationWarning", pending_deprecation_warning),
        ("RuntimeWarning", runtime_warning),
        ("SyntaxWarning", syntax_warning),
        ("ResourceWarning", resource_warning),
        ("FutureWarning", future_warning),
        ("ImportWarning", import_warning),
        ("UnicodeWarning", unicode_warning),
        ("BytesWarning", bytes_warning),
        ("EncodingWarning", encoding_warning),
        ("SystemExit", system_exit),
        ("GeneratorExit", generator_exit),
        ("KeyboardInterrupt", keyboard_interrupt),
        ("BaseExceptionGroup", base_exception_group),
        ("ExceptionGroup", exception_group),
    ]
}

thread_local! {
    /// Per-thread cache of all built-in exception class `Rc`s.
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

/// Return a clone of the thread-local `builtins` module.  O(1) on subsequent
/// calls — clones the `Rc<RefCell<PyModule>>` reference (attrs map is shared).
///
/// On the very first call per thread, applies post-processing to populate
/// the module attrs with:
///   - Primitive type `PyClass` singletons (`int`, `str`, `list`, …), replacing
///     the `BuiltinFunction(name)` tokens that `pyrust_module!` emits.
///   - Built-in exception class `PyClass` singletons (`ValueError`, `TypeError`,
///     …), which CPython exposes as attributes of the `builtins` module
///     (issue #1255).
///
/// The mutation is applied once and is shared by all future callers because
/// every `Value::clone` of a `PyModule` shares the same `Rc<RefCell<PyModule>>`.
pub(crate) fn cached_builtins_module() -> Value {
    BUILTINS_MODULE_CACHE.with(|module| {
        // Lazily apply post-processing on first access: replace auto-generated
        // `BuiltinFunction("int")` tokens with real `PyClass` singletons, and
        // insert exception classes missing from the auto-generated module.
        // Guard: if "int" is already a PyClass, post-processing already ran.
        let already_processed = if let ValueKind::PyModule(m) = module.kind() {
            matches!(
                m.borrow().attrs.get("int").map(|v| v.kind()),
                Some(ValueKind::PyClass(_))
            )
        } else {
            true
        };
        if !already_processed {
            if let ValueKind::PyModule(m) = module.kind() {
                let mut mod_attrs = m.borrow_mut();
                // Primitive types.
                for prim in [
                    "bool", "bytearray", "bytes", "complex", "dict", "float", "frozenset",
                    "int", "list", "set", "str", "tuple",
                ] {
                    if let Some(class) = primitive_class_by_name(prim) {
                        mod_attrs.attrs.insert(prim.to_string(), Value::py_class(class));
                    }
                }
                // `type` metaclass (issue #1312): must display as `<class 'type'>`.
                mod_attrs.attrs.insert(
                    "type".to_string(),
                    Value::py_class(type_class_singleton()),
                );
                // `object` (issue #1313): must display as `<class 'object'>`.
                mod_attrs.attrs.insert(
                    "object".to_string(),
                    Value::py_class(object_class_singleton()),
                );
                // Built-in exception classes (issue #1255).  Skip names with '.'
                // (e.g. "io.UnsupportedOperation") — those belong to other modules.
                // Also skip bare names that are registered under a dotted alias
                // (e.g. "UnsupportedOperation" is an io-module class even though
                // its short key has no dot); detect this by checking whether any
                // dotted key in the map points to the same Rc allocation.
                let exc_map = build_exc_class_map();
                let non_builtin_ptrs: std::collections::HashSet<*const _> = exc_map
                    .iter()
                    .filter(|(n, _)| n.contains('.'))
                    .map(|(_, cls)| Rc::as_ptr(cls))
                    .collect();
                for (exc_name, exc_class) in &exc_map {
                    if !exc_name.contains('.')
                        && !non_builtin_ptrs.contains(&Rc::as_ptr(exc_class))
                    {
                        mod_attrs.attrs.insert(
                            exc_name.to_string(),
                            Value::py_class(Rc::clone(exc_class)),
                        );
                    }
                }
            }
        }
        Value::clone(module)
    })
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

/// Return the `__doc__` string for a built-in type or exception class by name.
/// Returns `None` if the name is not a known built-in; callers should return
/// `Value::none()` in that case to match CPython's behaviour for unknown types.
pub(crate) fn builtin_class_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        // object
        "object" => "The base class of the class hierarchy.\n\nWhen called, it accepts no arguments and returns a new featureless\ninstance that has no instance attributes and cannot be given any.\n",
        // Primitive types
        "int" => "int([x]) -> integer\nint(x, base=10) -> integer\n\nConvert a number or string to an integer, or return 0 if no arguments\nare given.  If x is a number, return x.__int__().  For floating point\nnumbers, this truncates towards zero.\n\nIf x is not a number or if base is given, then x must be a string,\nbytes, or bytearray instance representing an integer literal in the\ngiven base.  The literal can be preceded by '+' or '-' and be surrounded\nby whitespace.  The base defaults to 10.  Valid bases are 0 and 2-36.\nBase 0 means to interpret the base from the string as an integer literal.\n>>> int('0b100', base=0)\n4",
        "str" => "str(object='') -> str\nstr(bytes_or_buffer[, encoding[, errors]]) -> str\n\nCreate a new string object from the given object. If encoding or\nerrors is specified, then the object must expose a data buffer\nthat will be decoded using the given encoding and error handler.\nOtherwise, returns the result of object.__str__() (if defined)\nor repr(object).\nencoding defaults to sys.getdefaultencoding().\nerrors defaults to 'strict'.",
        "list" => "Built-in mutable sequence.\n\nIf no argument is given, the constructor creates a new empty list.\nThe argument must be an iterable if specified.",
        "dict" => "dict() -> new empty dictionary\ndict(mapping) -> new dictionary initialized from a mapping object's\n    (key, value) pairs\ndict(iterable) -> new dictionary initialized as if via:\n    d = {}\n    for k, v in iterable:\n        d[k] = v\ndict(**kwargs) -> new dictionary initialized with the name=value pairs\n    in the keyword argument list.  For example:  dict(one=1, two=2)",
        "tuple" => "Built-in immutable sequence.\n\nIf no argument is given, the constructor returns an empty tuple.\nIf iterable is specified the tuple is initialized from iterable's items.\n\nIf the argument is a tuple, the return value is the same object.",
        "set" => "set() -> new empty set object\nset(iterable) -> new set object\n\nBuild an unordered collection of unique elements.",
        "frozenset" => "frozenset() -> empty frozenset object\nfrozenset(iterable) -> frozenset object\n\nBuild an immutable unordered collection of unique elements.",
        "bytes" => "bytes(iterable_of_ints) -> bytes\nbytes(string, encoding[, errors]) -> bytes\nbytes(bytes_or_buffer) -> immutable copy of bytes_or_buffer\nbytes(int) -> bytes object of size given by the parameter initialized with null bytes\nbytes() -> empty bytes object\n\nConstruct an immutable array of bytes from:\n  - an iterable yielding integers in range(256)\n  - a text string encoded using the specified encoding\n  - any object implementing the buffer API.\n  - an integer",
        "float" => "Convert a string or number to a floating point number, if possible.",
        "bool" => "bool(x) -> bool\n\nReturns True when the argument x is true, False otherwise.\nThe builtins True and False are the only two instances of the class bool.\nThe class bool is a subclass of the class int, and cannot be subclassed.",
        "complex" => "Create a complex number from a real part and an optional imaginary part.\n\nThis is equivalent to (real + imag*1j) where imag defaults to 0.",
        // Exception classes
        "BaseException" => "Common base class for all exceptions",
        "Exception" => "Common base class for all non-exit exceptions.",
        "ArithmeticError" => "Base class for arithmetic errors.",
        "LookupError" => "Base class for lookup errors.",
        "ValueError" => "Inappropriate argument value (of correct type).",
        "TypeError" => "Inappropriate argument type.",
        "NameError" => "Name not found globally.",
        "UnboundLocalError" => "Local name referenced before assignment.",
        "AttributeError" => "Attribute not found.",
        "KeyError" => "Mapping key not found.",
        "IndexError" => "Sequence index out of range.",
        "OverflowError" => "Result too large to be represented.",
        "ZeroDivisionError" => "Second argument to a division or modulo operation was zero.",
        "FloatingPointError" => "Floating point operation failed.",
        "RuntimeError" => "Unspecified run-time error.",
        "RecursionError" => "Recursion limit exceeded.",
        "NotImplementedError" => "Method or function hasn't been implemented yet.",
        "AssertionError" => "Assertion failed.",
        "StopIteration" => "Signal the end from iterator.__next__().",
        "EOFError" => "Read beyond end of file.",
        "MemoryError" => "Out of memory.",
        "ImportError" => "Import can't find module, or can't find name in module.",
        "ModuleNotFoundError" => "Module not found.",
        "UnicodeError" => "Unicode related error.",
        "UnicodeEncodeError" => "Unicode encoding error.",
        "UnicodeDecodeError" => "Unicode decoding error.",
        "UnicodeTranslateError" => "Unicode translation error.",
        "BufferError" => "Buffer error.",
        "ReferenceError" => "Weak ref proxy used after referent went away.",
        "SystemError" => "Internal error in the Python interpreter.\n\nPlease report this to the Python maintainer, along with the traceback,\nthe Python version, and the hardware/OS platform and version.",
        "StopAsyncIteration" => "Signal the end from iterator.__anext__().",
        "SyntaxError" => "Invalid syntax.",
        "IndentationError" => "Improper indentation.",
        "TabError" => "Improper mixture of spaces and tabs.",
        "OSError" => "Base class for I/O related errors.",
        "FileNotFoundError" => "File not found.",
        "FileExistsError" => "File already exists.",
        "BlockingIOError" => "I/O operation would block.",
        "ChildProcessError" => "Child process error.",
        "InterruptedError" => "Interrupted by signal.",
        "IsADirectoryError" => "Operation doesn't support directories.",
        "NotADirectoryError" => "Operation only works on directories.",
        "PermissionError" => "Not enough permissions.",
        "ProcessLookupError" => "Process not found.",
        "TimeoutError" => "Timeout expired.",
        "ConnectionError" => "Connection error.",
        "BrokenPipeError" => "Broken pipe.",
        "ConnectionAbortedError" => "Connection aborted.",
        "ConnectionRefusedError" => "Connection refused.",
        "ConnectionResetError" => "Connection reset.",
        "UnsupportedOperation" => "Operation not supported on this file type.",
        "Warning" => "Base class for warning categories.",
        "UserWarning" => "Base class for warnings generated by user code.",
        "DeprecationWarning" => "Base class for warnings about deprecated features.",
        "PendingDeprecationWarning" => "Base class for warnings about features which will be deprecated\nin the future.",
        "RuntimeWarning" => "Base class for warnings about dubious runtime behavior.",
        "SyntaxWarning" => "Base class for warnings about dubious syntax.",
        "ResourceWarning" => "Base class for warnings about resource usage.",
        "FutureWarning" => "Base class for warnings about constructs that will change semantically\nin the future.",
        "ImportWarning" => "Base class for warnings about probable mistakes in module imports.",
        "UnicodeWarning" => "Base class for warnings about Unicode related problems, mostly\nrelated to conversion problems.",
        "BytesWarning" => "Base class for warnings about bytes and buffer related problems, mostly\nrelated to conversion from str or comparing to str.",
        "EncodingWarning" => "Base class for warnings about encodings.",
        "SystemExit" => "Request to exit from the interpreter.",
        "GeneratorExit" => "Request that a generator exit.",
        "KeyboardInterrupt" => "Program interrupted by user.",
        "BaseExceptionGroup" => "A combination of multiple unrelated exceptions.",
        "ExceptionGroup" => "A combination of multiple unrelated exceptions.",
        _ => return None,
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
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(items) => {
            let mut set: PySet = PySet::default();
            for k in items {
                set.insert(k);
            }
            pyrust_builtins::frozenset::frozenset(set)
        }
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Bytes(rc) => Value::bytes((*rc).clone()),
        PyKey::Complex(re, im) => Value::complex(re, im),
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
    dict: &mut PyDict,
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
) -> PyDict {
    let mut dict: PyDict = PyDict::default();
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

// ─────────────────────────────────────────────────────────────────────────────
// Name resolution — the env-lookup rule (issue #452)
// ─────────────────────────────────────────────────────────────────────────────
//
// `Environment` (pyrust-core) carries TWO parallel stores for the same
// conceptual entity (a scope's bindings):
//
//   * `values: HashMap<String, Value>` — the name-keyed slow path. Holds
//     module/class-body bindings, closure/`nonlocal` cells, and any function
//     local the compiler chose NOT to put in a register.
//   * fastlocals (register file) — index-keyed fast path. NOT a field of
//     `Environment`; it lives in the active VM frame (`VmFrameView` /
//     `regs_ptr`). The compiler assigns each fastlocal a slot via
//     scope analysis (`local_index`). Most function locals live here.
//
// The compiler decides at compile time which store each name uses; the
// runtime never has to guess. The dispatch is keyed by the per-scope name
// sets on `Environment` (`global_names`, `nonlocal_names`, `local_names`)
// plus whether the env is the module/root env (`parent.is_none()`).
//
// THE RULE (single source of truth — keep this and `Interpreter::lookup_name`
// / `Interpreter::assign_name` in env.rs in agreement):
//
//   1. Name in `global_names` (a `global x` declaration in scope):
//        read  -> `lookup_name_in_module`: module-env HashMap, then builtin
//                 exception classes.
//        write -> module-env HashMap (+ live globals dict / module fastlocal
//                 register mirror). See `assign_name`'s `is_global` arm.
//
//   2. Name in `nonlocal_names` (a `nonlocal x` declaration in scope):
//        read  -> `lookup_name_in_enclosing_local_env`: walk to the nearest
//                 ENCLOSING function scope that declares `x` local, read there.
//        write -> `env_assign_local` into that same enclosing env.
//
//   3. Otherwise (ordinary local / free-variable read):
//        read  -> `lookup_name_in_env`: this env's HashMap, raising
//                 `UnboundLocalError` if `x` is a declared local of THIS scope
//                 but currently unset, otherwise recursing into parents
//                 (free-variable capture), bottoming out at the module env.
//        write -> `env_assign_local` into the current env (module-scope writes
//                 also mirror into the globals dict / bump the LoadGlobal
//                 inline-cache version).
//
// The fastlocal register path is orthogonal: when a name HAS a fastlocal slot
// in the active frame, the compiler addresses it directly as a register
// operand (`Insn::Move`, `Insn::BinOp`, … with the slot index) and never
// reaches these helpers at all. These helpers are the env-HashMap (slow) side
// of the duality; the name-keyed opcodes `Insn::LoadGlobal` / `Insn::StoreGlobal`
// fall through to them only for names without a register slot.
//
// Parent-chain scanning for rule 2 shares one body,
// `find_function_scope_with_local` (below). The `nonlocal` READ path
// (`lookup_name_in_enclosing_local_env`) skips the current env
// (`include_self == false`), while the `nonlocal` binding-existence CHECK
// performed at function-definition time (`has_local_binding_in_current_or_ancestor`,
// rejecting `nonlocal x` with no enclosing binding) includes it
// (`include_self == true`), since the defining env may itself be the binding scope.
//
// #384 (class body cannot resolve module-scope names) would integrate here:
// a class-body env currently follows rule 3 and bottoms out at the module env,
// but class bodies should resolve FREE names directly against module scope
// (skipping intervening function locals). That fix belongs in
// `lookup_name_in_env`'s parent-walk / a class-body-specific arm; it is OUT OF
// SCOPE for #452 and intentionally not changed here.
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

/// Walk the env parent chain for the first **function scope**
/// (`parent.is_some()`, i.e. not the module/root env) that declares `name`
/// as one of its locals (`local_names.contains(name)`), and return that env.
///
/// `include_self` controls the start point: `true` begins the scan at `env`
/// itself, `false` begins at `env`'s parent.  The module/root env can never
/// match because it has no parent — module-scope names live in the module-env
/// HashMap and are reached via [`lookup_name_in_module`], not this walk.
///
/// This is the single shared scan body for both the `nonlocal`-resolution
/// READ path ([`find_enclosing_local_env_for_name`], `include_self == false`,
/// skips the current env to find the enclosing binding scope) and the
/// `nonlocal` binding-existence CHECK at function-definition time
/// ([`has_local_binding_in_current_or_ancestor`], `include_self == true`,
/// since the defining env may itself declare the binding).
/// The two callers differ only in start point and return shape; the matching
/// predicate (`parent.is_some() && local_names.contains(name)`) is identical.
fn find_function_scope_with_local(env: &EnvRef, name: &str, include_self: bool) -> Option<EnvRef> {
    let mut current = if include_self {
        Some(Rc::clone(env))
    } else {
        env.borrow().parent.clone()
    };
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

fn has_local_binding_in_current_or_ancestor(env: &EnvRef, name: &str) -> bool {
    find_function_scope_with_local(env, name, true).is_some()
}

/// Resolve `name` to its captured value in the **non-module** portion of the
/// `env` chain (issue #2106).  Walks from `env` (the function's captured
/// enclosing scope) outward, returning the first `values` entry for `name`
/// found in a function scope (`parent.is_some()`); the module/root env is never
/// consulted, so a true module global returns `None` and is not reported as a
/// closure free variable.  Used by `closure_free_vars` to build `__closure__`
/// cells and `co_freevars`.
pub(crate) fn lookup_enclosing_function_value(env: &EnvRef, name: &str) -> Option<Value> {
    let mut current = Some(Rc::clone(env));
    while let Some(candidate) = current {
        let (is_function_scope, value, next) = {
            let borrowed = candidate.borrow();
            (
                borrowed.parent.is_some(),
                borrowed.values.get(name).cloned(),
                borrowed.parent.clone(),
            )
        };
        if is_function_scope {
            if let Some(v) = value {
                if !v.is_unset() {
                    return Some(v);
                }
            }
        }
        current = next;
    }
    None
}

fn find_enclosing_local_env_for_name(env: &EnvRef, name: &str) -> Option<EnvRef> {
    find_function_scope_with_local(env, name, false)
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
            Stmt::Assign(target, rhs) => {
                collect_assign_target_names(target, names, global_names, nonlocal_names);
                // Walrus targets inside comprehensions on the RHS escape to this
                // function's scope (PEP 572).
                collect_walrus_targets_in_expr(rhs, names, global_names, nonlocal_names);
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
            Stmt::AnnAssign { name, value, .. } => {
                // Both `x: T = v` (value = Some) and `x: T` (value = None) declare
                // a local slot.  At function scope the bare form causes UnboundLocalError
                // on read (matching CPython); at class scope the slot is allocated but
                // never stored via RecordClassStore so it does not appear in vars(C).
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
                // Walrus targets inside a comprehension on the RHS escape to this
                // function's scope (PEP 572).
                if let Some(v) = value {
                    collect_walrus_targets_in_expr(v, names, global_names, nonlocal_names);
                }
            }
            Stmt::AugAssign { expr, .. } => {
                // Walrus targets inside a comprehension on the RHS escape to this
                // function's scope (PEP 572).
                collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
            }
            Stmt::IndexAssign { expr, .. } | Stmt::SliceAssign { expr, .. } => {
                // Walrus targets inside a comprehension on the RHS escape to this
                // function's scope (PEP 572).
                collect_walrus_targets_in_expr(expr, names, global_names, nonlocal_names);
            }
            Stmt::Raise { expr, cause } => {
                // Walrus targets inside a comprehension in the raise expression or
                // cause escape to this function's scope (PEP 572).
                if let Some(e) = expr {
                    collect_walrus_targets_in_expr(e, names, global_names, nonlocal_names);
                }
                if let Some(c) = cause {
                    collect_walrus_targets_in_expr(c, names, global_names, nonlocal_names);
                }
            }
            Stmt::Delete(_) | Stmt::Break | Stmt::Continue | Stmt::Pass => {}
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
            Stmt::With { items, body, .. } => {
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
                ..
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
                ..
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
                ..
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
                ..
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
            // `type X[T] = expr` binds `X` as a local name (PEP 695).
            // The type params (T) are NOT local names — they are temporaries
            // visible only during RHS evaluation and do not escape to the scope.
            Stmt::TypeAlias { name, value, .. } => {
                if !global_names.contains(name) && !nonlocal_names.contains(name) {
                    names.insert(name.clone());
                }
                collect_walrus_targets_in_expr(value, names, global_names, nonlocal_names);
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
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Value(_) => {}
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
        Pattern::Class {
            positional, kwargs, ..
        } => {
            for pat in positional {
                collect_pattern_names(pat, names, global_names, nonlocal_names);
            }
            for (_, attr_pat) in kwargs {
                collect_pattern_names(attr_pat, names, global_names, nonlocal_names);
            }
        }
        Pattern::As { pattern, name } => {
            collect_pattern_names(pattern, names, global_names, nonlocal_names);
            if !global_names.contains(name) && !nonlocal_names.contains(name) {
                names.insert(name.clone());
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
            Stmt::If { branches, else_branch, .. } => {
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
            Stmt::Try { body, handlers, else_branch, finally_branch, .. } => {
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
            Stmt::If { branches, else_branch, .. } => {
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
            Stmt::Try { body, handlers, else_branch, finally_branch, .. } => {
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

/// Check that no `global x` or `nonlocal x` declaration in `body` (at any
/// nesting depth within the same function scope) appears after a prior
/// assignment to or use of `x` in that same scope.
///
/// CPython 3.12 raises two distinct SyntaxError messages:
/// - `"name 'x' is assigned to before global declaration"` — when `x` was
///   bound (assigned, for-target, with-target, def, class, AugAssign, walrus)
///   before the `global x` declaration.
/// - `"name 'x' is used prior to global declaration"` — when `x` was read
///   (appeared as `Expr::Var`) but not bound before the `global x`.
///
/// The same messages apply for `nonlocal`, substituting "nonlocal" for
/// "global".
///
/// Returns `Some(error_message)` on the first violation found, or `None`.
pub(crate) fn check_global_nonlocal_order(body: &[Stmt]) -> Option<String> {
    let mut assigned: HashSet<String> = HashSet::new();
    let mut used: HashSet<String> = HashSet::new();
    check_global_nonlocal_order_block(body, &mut assigned, &mut used)
}

/// Recursive helper: walk `stmts` in order, updating `assigned` and `used`,
/// and returning an error on the first ordering violation.
fn check_global_nonlocal_order_block(
    stmts: &[Stmt],
    assigned: &mut HashSet<String>,
    used: &mut HashSet<String>,
) -> Option<String> {
    for stmt in stmts {
        match stmt {
            Stmt::Global(names) => {
                for name in names {
                    // CPython checks "used" before "assigned": when both sets
                    // contain the name (e.g. `x = 1; print(x); global x`),
                    // CPython always reports "used prior to global declaration".
                    if used.contains(name) {
                        return Some(format!(
                            "name '{}' is used prior to global declaration",
                            name
                        ));
                    }
                    if assigned.contains(name) {
                        return Some(format!(
                            "name '{}' is assigned to before global declaration",
                            name
                        ));
                    }
                }
            }
            Stmt::Nonlocal(names) => {
                for name in names {
                    // Same priority: "used" wins over "assigned" (matches CPython).
                    if used.contains(name) {
                        return Some(format!(
                            "name '{}' is used prior to nonlocal declaration",
                            name
                        ));
                    }
                    if assigned.contains(name) {
                        return Some(format!(
                            "name '{}' is assigned to before nonlocal declaration",
                            name
                        ));
                    }
                }
            }
            // Assignments bind names.
            Stmt::Assign(target, expr) => {
                collect_var_refs_in_expr(expr, used, assigned);
                collect_assign_target_bound_names(target, assigned);
            }
            Stmt::AugAssign { target, expr, .. } => {
                // AugAssign reads the target first, then writes it.
                // Both count as "assigned to" since the target is also bound.
                collect_var_refs_in_expr(expr, used, assigned);
                collect_assign_target_bound_names(target, assigned);
            }
            Stmt::AnnAssign { annotation, value, .. } => {
                // AnnAssign (`x: T` or `x: T = v`) is handled by the separate
                // "annotated name can't be global/nonlocal" check, which is
                // order-independent and always produces "annotated name 'x'
                // can't be global" regardless of order.  Skip the target name
                // here so we don't produce a conflicting message.
                collect_var_refs_in_expr(annotation, used, assigned);
                if let Some(v) = value {
                    collect_var_refs_in_expr(v, used, assigned);
                }
            }
            Stmt::Def { name, decorators, .. } => {
                // Decorators are evaluated in the outer scope.
                for dec in decorators {
                    collect_var_refs_in_expr(dec, used, assigned);
                }
                // The def name is bound in the outer scope (not recursed into).
                assigned.insert(name.clone());
            }
            Stmt::Class { name, bases, metaclass, keywords, decorators, .. } => {
                for dec in decorators {
                    collect_var_refs_in_expr(dec, used, assigned);
                }
                for base in bases {
                    collect_var_refs_in_expr(base, used, assigned);
                }
                if let Some(mc) = metaclass {
                    collect_var_refs_in_expr(mc, used, assigned);
                }
                for (_, kw) in keywords {
                    collect_var_refs_in_expr(kw, used, assigned);
                }
                assigned.insert(name.clone());
            }
            Stmt::For { target, iter, body, else_branch, .. } => {
                collect_var_refs_in_expr(iter, used, assigned);
                collect_assign_target_bound_names(target, assigned);
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
                if let Some(branch) = else_branch {
                    if let Some(msg) =
                        check_global_nonlocal_order_block(branch, assigned, used)
                    {
                        return Some(msg);
                    }
                }
            }
            Stmt::With { items, body, .. } => {
                for (expr, alias) in items {
                    collect_var_refs_in_expr(expr, used, assigned);
                    if let Some(target) = alias {
                        collect_assign_target_bound_names(target, assigned);
                    }
                }
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
            }
            Stmt::Expr(e) => {
                collect_var_refs_in_expr(e, used, assigned);
            }
            Stmt::Return(Some(e)) => {
                collect_var_refs_in_expr(e, used, assigned);
            }
            Stmt::Return(None) => {}
            Stmt::If { branches, else_branch, .. } => {
                for (cond, branch) in branches {
                    collect_var_refs_in_expr(cond, used, assigned);
                    if let Some(msg) =
                        check_global_nonlocal_order_block(branch, assigned, used)
                    {
                        return Some(msg);
                    }
                }
                if let Some(branch) = else_branch {
                    if let Some(msg) =
                        check_global_nonlocal_order_block(branch, assigned, used)
                    {
                        return Some(msg);
                    }
                }
            }
            Stmt::While { cond, body, else_branch, .. } => {
                collect_var_refs_in_expr(cond, used, assigned);
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
                if let Some(branch) = else_branch {
                    if let Some(msg) =
                        check_global_nonlocal_order_block(branch, assigned, used)
                    {
                        return Some(msg);
                    }
                }
            }
            Stmt::Try { body, handlers, else_branch, finally_branch, .. } => {
                if let Some(msg) = check_global_nonlocal_order_block(body, assigned, used) {
                    return Some(msg);
                }
                for handler in handlers {
                    if let Some(bound) = &handler.name {
                        assigned.insert(bound.clone());
                    }
                    if let Some(exc_type) = &handler.kind {
                        collect_var_refs_in_expr(exc_type, used, assigned);
                    }
                    if let Some(msg) =
                        check_global_nonlocal_order_block(&handler.body, assigned, used)
                    {
                        return Some(msg);
                    }
                }
                if let Some(branch) = else_branch {
                    if let Some(msg) =
                        check_global_nonlocal_order_block(branch, assigned, used)
                    {
                        return Some(msg);
                    }
                }
                if let Some(branch) = finally_branch {
                    if let Some(msg) =
                        check_global_nonlocal_order_block(branch, assigned, used)
                    {
                        return Some(msg);
                    }
                }
            }
            Stmt::Match { subject, arms } => {
                collect_var_refs_in_expr(subject, used, assigned);
                for arm in arms {
                    collect_pattern_bound_names(&arm.pattern, assigned);
                    if let Some(guard) = &arm.guard {
                        collect_var_refs_in_expr(guard, used, assigned);
                    }
                    if let Some(msg) =
                        check_global_nonlocal_order_block(&arm.body, assigned, used)
                    {
                        return Some(msg);
                    }
                }
            }
            Stmt::Delete(exprs) => {
                for e in exprs {
                    collect_var_refs_in_expr(e, used, assigned);
                }
            }
            Stmt::Assert { test, msg } => {
                collect_var_refs_in_expr(test, used, assigned);
                if let Some(m) = msg {
                    collect_var_refs_in_expr(m, used, assigned);
                }
            }
            Stmt::Raise { expr, cause } => {
                if let Some(e) = expr {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                if let Some(c) = cause {
                    collect_var_refs_in_expr(c, used, assigned);
                }
            }
            // Import/ImportFrom do NOT trigger "used prior to" — CPython does
            // not flag `import x; global x` as a SyntaxError.
            Stmt::Import { .. } | Stmt::ImportFrom { .. } => {}
            Stmt::AttrAssign { target, expr, .. } => {
                collect_var_refs_in_expr(target, used, assigned);
                collect_var_refs_in_expr(expr, used, assigned);
            }
            Stmt::IndexAssign { target, index, expr } => {
                collect_var_refs_in_expr(target, used, assigned);
                collect_var_refs_in_expr(index, used, assigned);
                collect_var_refs_in_expr(expr, used, assigned);
            }
            Stmt::SliceAssign { target, lower, upper, step, expr } => {
                collect_var_refs_in_expr(target, used, assigned);
                if let Some(e) = lower {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                if let Some(e) = upper {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                if let Some(e) = step {
                    collect_var_refs_in_expr(e, used, assigned);
                }
                collect_var_refs_in_expr(expr, used, assigned);
            }
            Stmt::Break | Stmt::Continue | Stmt::Pass => {}
            // `type X[T] = expr` binds X; references in expr are "used".
            Stmt::TypeAlias { name, value, .. } => {
                collect_var_refs_in_expr(value, used, assigned);
                assigned.insert(name.clone());
            }
        }
    }
    None
}

/// Collect all `Var(name)` references from an expression into `used`, and
/// walrus-operator binding targets into `assigned`.
/// Does NOT descend into nested function scopes (Def, Lambda, comprehensions).
fn collect_var_refs_in_expr(
    expr: &Expr,
    used: &mut HashSet<String>,
    assigned: &mut HashSet<String>,
) {
    match expr {
        Expr::Var(name) => {
            used.insert(name.clone());
        }
        Expr::Named { target, value } => {
            // Walrus operator — the target is a binding in the outer scope
            // ("assigned to"), not merely a use.
            assigned.insert(target.clone());
            collect_var_refs_in_expr(value, used, assigned);
        }
        // Do NOT descend into nested scopes — they have their own symbol table.
        Expr::Lambda { .. }
        | Expr::ListComp { .. }
        | Expr::SetComp { .. }
        | Expr::DictComp { .. }
        | Expr::GenExp { .. } => {}
        // Recurse into sub-expressions.
        Expr::List(elts) | Expr::Tuple(elts) | Expr::Set(elts) => {
            for e in elts {
                collect_var_refs_in_expr(e, used, assigned);
            }
        }
        Expr::Dict(items) => {
            for item in items {
                match item {
                    crate::ast::DictItem::Pair(k, v) => {
                        collect_var_refs_in_expr(k, used, assigned);
                        collect_var_refs_in_expr(v, used, assigned);
                    }
                    crate::ast::DictItem::DoubleSplat(e) => {
                        collect_var_refs_in_expr(e, used, assigned);
                    }
                }
            }
        }
        Expr::Unary { expr, .. } => collect_var_refs_in_expr(expr, used, assigned),
        Expr::Binary { left, right, .. } => {
            collect_var_refs_in_expr(left, used, assigned);
            collect_var_refs_in_expr(right, used, assigned);
        }
        Expr::Compare { left, ops } => {
            collect_var_refs_in_expr(left, used, assigned);
            for (_, e) in ops {
                collect_var_refs_in_expr(e, used, assigned);
            }
        }
        Expr::Ternary { cond, then, else_ } => {
            collect_var_refs_in_expr(cond, used, assigned);
            collect_var_refs_in_expr(then, used, assigned);
            collect_var_refs_in_expr(else_, used, assigned);
        }
        Expr::Call { func, args } => {
            collect_var_refs_in_expr(func, used, assigned);
            for arg in args {
                collect_var_refs_in_expr(&arg.value, used, assigned);
            }
        }
        Expr::Attr { target, .. } => collect_var_refs_in_expr(target, used, assigned),
        Expr::Index { target, index } => {
            collect_var_refs_in_expr(target, used, assigned);
            collect_var_refs_in_expr(index, used, assigned);
        }
        Expr::Slice { target, lower, upper, step } => {
            collect_var_refs_in_expr(target, used, assigned);
            if let Some(e) = lower {
                collect_var_refs_in_expr(e, used, assigned);
            }
            if let Some(e) = upper {
                collect_var_refs_in_expr(e, used, assigned);
            }
            if let Some(e) = step {
                collect_var_refs_in_expr(e, used, assigned);
            }
        }
        Expr::Starred(e) => collect_var_refs_in_expr(e, used, assigned),
        Expr::FString(parts) => {
            for part in parts {
                if let crate::ast::FStringPart::Expr { expr, format_spec, .. } = part {
                    collect_var_refs_in_expr(expr, used, assigned);
                    if let Some(spec_parts) = format_spec {
                        for spec_part in spec_parts {
                            if let crate::ast::FStringPart::Expr { expr: spec_expr, .. } =
                                spec_part
                            {
                                collect_var_refs_in_expr(spec_expr, used, assigned);
                            }
                        }
                    }
                }
            }
        }
        Expr::Yield(Some(e)) => collect_var_refs_in_expr(e, used, assigned),
        Expr::YieldFrom(e) => collect_var_refs_in_expr(e, used, assigned),
        Expr::Await(e) => collect_var_refs_in_expr(e, used, assigned),
        // Literals / constants — no names.
        Expr::Int(_)
        | Expr::BigInt(_)
        | Expr::Float(_)
        | Expr::Complex(_, _)
        | Expr::Str(_)
        | Expr::Bytes(_)
        | Expr::Bool(_)
        | Expr::None
        | Expr::Ellipsis
        | Expr::Yield(None) => {}
    }
}

/// Collect the names **bound** by an assignment target (left-hand side of `=`
/// or a `for`/`with` binding target).  Only `Name` targets are collected;
/// attribute and index targets do not introduce new local bindings.
fn collect_assign_target_bound_names(target: &AssignTarget, assigned: &mut HashSet<String>) {
    match target {
        AssignTarget::Name(n) => {
            assigned.insert(n.clone());
        }
        AssignTarget::Tuple(targets) => {
            for t in targets {
                collect_assign_target_bound_names(t, assigned);
            }
        }
        AssignTarget::Starred(inner) => {
            collect_assign_target_bound_names(inner, assigned);
        }
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
    }
}

/// Collect names bound by a match pattern (capture patterns bind names).
fn collect_pattern_bound_names(pattern: &crate::ast::Pattern, assigned: &mut HashSet<String>) {
    match pattern {
        crate::ast::Pattern::Capture(name) => {
            assigned.insert(name.clone());
        }
        crate::ast::Pattern::Or(pats) => {
            for p in pats {
                collect_pattern_bound_names(p, assigned);
            }
        }
        crate::ast::Pattern::Sequence(elts) => {
            for (p, _) in elts {
                collect_pattern_bound_names(p, assigned);
            }
        }
        crate::ast::Pattern::Mapping(pairs, rest) => {
            for (_, p) in pairs {
                collect_pattern_bound_names(p, assigned);
            }
            if let Some(name) = rest {
                assigned.insert(name.clone());
            }
        }
        crate::ast::Pattern::Class {
            positional, kwargs, ..
        } => {
            for p in positional {
                collect_pattern_bound_names(p, assigned);
            }
            for (_, p) in kwargs {
                collect_pattern_bound_names(p, assigned);
            }
        }
        crate::ast::Pattern::Wildcard
        | crate::ast::Pattern::Literal(_)
        | crate::ast::Pattern::Value(_) => {}
        crate::ast::Pattern::As { pattern, name } => {
            collect_pattern_bound_names(pattern, assigned);
            assigned.insert(name.clone());
        }
    }
}

fn values_are_identical(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::None, ValueKind::None) => true,
        (ValueKind::NotImplemented, ValueKind::NotImplemented) => true,
        (ValueKind::Ellipsis, ValueKind::Ellipsis) => true,
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
        //
        // Special case: `instance_dict` proxies always have distinct BuiltinState
        // Rc's (a new proxy is created on each `vars(obj)` / `obj.__dict__`
        // access).  CPython guarantees `vars(obj) is vars(obj)` is `True` for the
        // same instance — we match that by checking whether both proxies point to
        // the same underlying PyInstance via `same_instance` (#1027).
        (
            ValueKind::BuiltinObject {
                ops: ops_a,
                state: sa,
            },
            ValueKind::BuiltinObject {
                ops: ops_b,
                state: sb,
            },
        ) => {
            if Rc::ptr_eq(sa, sb) {
                return true;
            }
            if ops_a.type_name() == pyrust_builtins::instance_dict::TYPE_NAME
                && ops_b.type_name() == pyrust_builtins::instance_dict::TYPE_NAME
            {
                return pyrust_builtins::instance_dict::same_instance(sa, sb);
            }
            false
        }
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
        // Walrus targets inside a comprehension escape to the nearest enclosing
        // non-comprehension scope (PEP 572). Descend into the element/value
        // expressions and filter conditions so their walrus targets are recorded
        // as locals of the enclosing function.  Do NOT descend into Lambda nodes
        // (they create a true new scope).
        Expr::ListComp { elt, clauses }
        | Expr::SetComp { elt, clauses }
        | Expr::GenExp { elt, clauses } => {
            collect_walrus_targets_in_expr(elt, names, global_names, nonlocal_names);
            for clause in clauses {
                if let Some(c) = &clause.cond {
                    collect_walrus_targets_in_expr(c, names, global_names, nonlocal_names);
                }
                // Inner clause iters also run in the comprehension scope and can
                // contain walrus expressions that escape outward.
                collect_walrus_targets_in_expr(&clause.iter, names, global_names, nonlocal_names);
            }
        }
        Expr::DictComp { key, val, clauses } => {
            collect_walrus_targets_in_expr(key, names, global_names, nonlocal_names);
            collect_walrus_targets_in_expr(val, names, global_names, nonlocal_names);
            for clause in clauses {
                if let Some(c) = &clause.cond {
                    collect_walrus_targets_in_expr(c, names, global_names, nonlocal_names);
                }
                collect_walrus_targets_in_expr(&clause.iter, names, global_names, nonlocal_names);
            }
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
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
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

/// Convert a finite `f64` to a `BigInt` `Value`, raising the CPython-matching
/// Python exceptions for non-finite inputs:
/// - `OverflowError` for ±infinity
/// - `ValueError` for NaN
///
/// For finite floats the fractional part is discarded (truncation toward zero),
/// matching CPython's `int(float)` / `math.floor` / `math.ceil` semantics.
pub(crate) fn float_to_bigint(f: f64) -> crate::error::Result<Value> {
    use crate::value::PyBigInt;
    use num_traits::FromPrimitive;
    if f.is_nan() {
        return Err(crate::error::PyError::named(
            "ValueError",
            "cannot convert float NaN to integer".to_string(),
        ));
    }
    if f.is_infinite() {
        return Err(crate::error::PyError::named(
            "OverflowError",
            "cannot convert float infinity to integer".to_string(),
        ));
    }
    // Truncate toward zero (matching CPython's behaviour for floor/ceil callers
    // that have already applied their own rounding before calling here).
    let n = PyBigInt::from_f64(f.trunc()).expect("finite f64 must convert to BigInt");
    Ok(Value::bigint(n))
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
        | Expr::None
        | Expr::Ellipsis => true,
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
        // yield/yield from/await always have side effects (suspension).
        Expr::Yield(_) | Expr::YieldFrom(_) | Expr::Await(_) => false,
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
        Stmt::If { branches, else_branch, .. } => {
            branches
                .iter()
                .all(|(cond, blk)| is_pure_expr(cond, pure_fns, local_names) && is_pure_body(blk, pure_fns, local_names))
                && else_branch
                    .as_deref()
                    .is_none_or(|b| is_pure_body(b, pure_fns, local_names))
        }
        Stmt::While { cond, body, else_branch, .. } => {
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
        Stmt::Try { body, handlers, else_branch, finally_branch, .. } => {
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
        // TypeAlias allocates a new heap object → impure (identity changes).
        Stmt::TypeAlias { .. } => false,
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

/// Round a float to the nearest integer (banker's rounding) for `round(x)`
/// with no `ndigits`, which returns an `int`.  Non-finite inputs cannot be
/// converted to an integer, so they raise the same errors as `int(float)` /
/// `math.floor` / `math.ceil`:
/// - `OverflowError("cannot convert float infinity to integer")` for ±inf
/// - `ValueError("cannot convert float NaN to integer")` for NaN
///
/// Finite inputs delegate to [`py_round_half_even`].
pub(crate) fn py_round_half_even_checked(v: f64) -> crate::error::Result<i64> {
    if v.is_nan() {
        return Err(crate::error::PyError::named(
            "ValueError",
            "cannot convert float NaN to integer".to_string(),
        ));
    }
    if v.is_infinite() {
        return Err(crate::error::PyError::named(
            "OverflowError",
            "cannot convert float infinity to integer".to_string(),
        ));
    }
    Ok(py_round_half_even(v))
}

/// Round an f64 to `ndigits` decimal places using CPython's half-even semantics.
///
/// CPython's `float.__round__(ndigits)` determines the rounding direction from the
/// float's **exact** rational value (via `_Py_dg_dtoa` internally), not from a
/// scaled intermediate float.  The naïve multiply-round-divide approach fails for
/// values like `round(2.675, 2)` because `2.675 * 100` may be exactly `267.5` in
/// f64, rounding the wrong way, even though the true exact value of the IEEE 754
/// float `2.675` is slightly *below* 2.675.
///
/// For `n >= 0` this function delegates to Rust's built-in `{:.prec$}` formatter,
/// which already uses the exact float value internally (Grisu3/Dragon4), then
/// parses the string back to f64.  NaN and infinities pass through as-is.
///
/// For `n < 0` (rounding to the nearest `10^(-n)`) the function uses big-integer
/// exact arithmetic: the float's mantissa and binary exponent are extracted from
/// the IEEE 754 bits, scaled to integers, and compared against the target factor
/// to determine the tie-breaking direction without any floating-point rounding.
/// NaN and infinities pass through as-is.
pub(crate) fn round_float_ndigits(v: f64, n: i32) -> crate::error::Result<Value> {
    if n >= 0 {
        // A f64 has at most 1074 significant decimal digits (the subnormal 5e-324
        // has exactly 324 significant decimal digits; normal floats have fewer).
        // Any ndigits > 1074 cannot change the float's value, so return v unchanged.
        // This cap also prevents Rust's formatter from panicking: format!("{:.prec$}")
        // panics when prec >= 65536, and ndigits_i32 can be as large as i32::MAX.
        if n > 1074 {
            return Ok(Value::float(v));
        }
        let prec = n as usize;
        // Rust's {:.prec$} formatter uses the exact float value (Grisu3/Dragon4),
        // so it correctly rounds 2.675 to 2.67 (the exact IEEE 754 value is slightly
        // below 2.675).  Parse back to f64 to recover the rounded float.
        // NaN and ±Inf format as "NaN" / "inf" / "-inf" and parse back unchanged.
        let s = format!("{:.prec$}", v, prec = prec);
        // parse() produces -0.0 for "-0.00" etc., matching CPython's sign semantics.
        let result: f64 = s.parse().unwrap_or(v);
        return Ok(Value::float(result));
    }

    // n < 0: round to nearest 10^(-n).  NaN and ±Inf pass through unchanged
    // (CPython: round(nan, -2) == nan, round(inf, -2) == inf).
    if !v.is_finite() {
        return Ok(Value::float(v));
    }

    // n < 0: round to nearest 10^(-n).  Use exact big-integer arithmetic.
    let neg_n = (-n) as u32;

    // 10^neg_n as a float: guard against factor overflow.
    let factor = 10f64.powf(neg_n as f64);
    if factor.is_infinite() {
        // 10^neg_n doesn't fit in f64 — any finite v rounds to signed zero.
        return Ok(Value::float(if v.is_sign_negative() {
            -0.0f64
        } else {
            0.0f64
        }));
    }

    // Decompose |v| = m * 2^e2  (exact IEEE 754 representation).
    let bits = v.to_bits();
    let sign_neg = (bits >> 63) != 0;
    let biased_exp = ((bits >> 52) & 0x7FF) as i32;
    let mantissa_bits = bits & ((1u64 << 52) - 1);

    // e2 = biased_exp - 1023 - 52  (normal); -1074 for subnormals.
    let (m_u64, e2): (u64, i32) = if biased_exp == 0 {
        (mantissa_bits, -1074)
    } else {
        (mantissa_bits | (1u64 << 52), biased_exp - 1075)
    };

    // v = sign * m_u64 * 2^e2.
    //
    // To avoid fractions: if e2 < 0, we scale both |v| and the factor by 2^(-e2).
    // Then:  |v| * 2^(-e2)  = m_u64           (exact integer)
    //        factor * 2^(-e2) = 10^neg_n * 2^(-e2)
    //
    // If e2 >= 0: |v| * 1 = m_u64 * 2^e2     (exact integer)
    //             factor * 1 = 10^neg_n
    //
    // In both cases the quotient |v| / factor equals v_num / factor_scaled exactly.
    let e2_neg = if e2 < 0 { (-e2) as u32 } else { 0u32 };

    let v_num: PyBigInt = if e2 >= 0 {
        let pow2 = PyPow::pow(PyBigInt::from(2u64), e2 as u32);
        PyBigInt::from(m_u64) * pow2
    } else {
        // e2 < 0: v_num = m (the 2^(-e2) scaling is absorbed into factor_scaled)
        PyBigInt::from(m_u64)
    };

    let factor_bigint = PyPow::pow(PyBigInt::from(10u64), neg_n);
    let factor_scaled: PyBigInt = if e2_neg > 0 {
        let pow2 = PyPow::pow(PyBigInt::from(2u64), e2_neg);
        factor_bigint * pow2
    } else {
        factor_bigint
    };

    // floor-divmod: v_num = q * factor_scaled + r,  0 <= r < factor_scaled.
    let (q, r) = bigint_divmod_floor(&v_num, &factor_scaled);

    // Compare 2*r to factor_scaled to determine half-even rounding direction.
    use num_traits::Zero;
    let two_r = &r + &r;
    use std::cmp::Ordering;
    let q_rounded: PyBigInt = match two_r.cmp(&factor_scaled) {
        Ordering::Less => q,
        Ordering::Greater => &q + &PyBigInt::from(1u64),
        Ordering::Equal => {
            // Exactly at the halfway point: round to even.
            if (&q % &PyBigInt::from(2u64)).is_zero() {
                q
            } else {
                &q + &PyBigInt::from(1u64)
            }
        }
    };

    // result = q_rounded * 10^neg_n as f64.
    // q_rounded is small (it is floor(|v| / 10^neg_n) ± 1), so converting to f64
    // via to_f64() is accurate for reasonable inputs.  The result is then multiplied
    // by the float factor (which is exact for powers of 10 that fit in f64).
    use num_traits::ToPrimitive;
    let q_f64 = q_rounded.to_f64().unwrap_or(f64::INFINITY);
    let result = q_f64 * factor;

    // Overflow check: if the rounded result doesn't fit in f64.
    if v.is_finite() && result.is_infinite() {
        return Err(PyError::named(
            "OverflowError",
            "rounded value too large to represent".to_string(),
        ));
    }

    // Sign: negative zero is preserved when |v| rounds to zero and v was negative.
    let result = if result == 0.0 {
        if sign_neg { -0.0f64 } else { 0.0f64 }
    } else if sign_neg {
        -result
    } else {
        result
    };

    Ok(Value::float(result))
}

/// Round a `PyBigInt` to the nearest `10^neg_n` using banker's rounding.
///
/// This implements CPython's `int.__round__(ndigits)` semantics for negative
/// `ndigits`: divide by `factor = 10^neg_n` using floor division, keep the
/// floor multiple, then apply half-even tie-breaking.  The result is returned
/// as `Value::int` if it fits in `i64`, otherwise `Value::bigint`.
///
/// Called by `round()` in builtins for `Int`, `Bool`, and `BigInt` inputs
/// when `ndigits` is negative.
pub(crate) fn round_bigint_neg_ndigits(x: PyBigInt, neg_n: u32) -> Value {
    use num_traits::ToPrimitive;

    // Early-exit: if 10^neg_n is so large that even the biggest possible
    // rounding (halfway up) can't reach the first non-zero multiple, the
    // result is always 0.  This prevents the hang that occurs when neg_n is
    // clamped from a large-negative BigInt ndigits to i32::MAX (~2 billion),
    // which would otherwise cause PyPow::pow(10, 2_147_483_647) to allocate
    // an ~850 MB intermediate value.  CPython returns 0 for this case too.
    //
    // A BigInt with D decimal digits satisfies |x| < 10^D, so:
    //   - At neg_n == D: rounding to 10^D is possible if |x| >= 5*10^(D-1).
    //   - At neg_n > D: |x| < 10^D < 10^neg_n / 10, which is always less
    //     than half = 10^neg_n / 2, so the rounded value is always 0.
    //
    // The exact decimal digit count via to_str_radix(10) is O(digits) but
    // this path is not hot (only reached for BigInt rounding).
    let decimal_digits = x.magnitude().to_str_radix(10).len() as u32;
    if neg_n > decimal_digits {
        return Value::int(0);
    }

    let factor = PyPow::pow(PyBigInt::from(10i64), neg_n);
    let half = &factor / PyBigInt::from(2i64);
    // floor-divmod: 0 ≤ r < factor, q = floor(x / factor)
    let (q, r) = bigint_divmod_floor(&x, &factor);
    let base = &q * &factor;
    let rounded = if r < half {
        base
    } else if r > half {
        base + &factor
    } else {
        // Tie: banker's rounding — round to even quotient.
        if (&q % PyBigInt::from(2i64)).is_zero() {
            base
        } else {
            base + &factor
        }
    };
    match rounded.to_i64() {
        Some(v) => Value::int(v),
        None => Value::bigint(rounded),
    }
}

/// Modular exponentiation: (base^exp) % modulus using BigInt arithmetic.
///
/// Callers MUST ensure exp >= 0 and modulus != 0 before calling; this
/// function panics if either precondition is violated (delegated to
/// BigInt::modpow).  The result is returned as Value::int when it fits in
/// i64, otherwise Value::bigint.
pub(crate) fn modpow_bigint(base: &PyBigInt, exp: &PyBigInt, modulus: &PyBigInt) -> Value {
    use num_traits::ToPrimitive;
    let result = base.modpow(exp, modulus);
    match result.to_i64() {
        Some(v) => Value::int(v),
        None => Value::bigint(result),
    }
}

/// Modular exponentiation: (base^exp) % modulus for i64.
///
/// Intermediate products are widened to i128 to prevent overflow when
/// `modulus` is large (up to ~2^62).  `(i64::MAX)^2 ≈ 2^126 < i128::MAX`,
/// so all intermediates fit exactly.  Issue #1697.
pub(crate) fn modpow_i64(base: i64, exp: u64, modulus: i64) -> i64 {
    if modulus == 1 {
        return 0;
    }
    let m = modulus as i128;
    let mut result: i128 = 1;
    let mut base = ((base as i128 % m) + m) % m;
    let mut exp = exp;
    while exp > 0 {
        if exp % 2 == 1 {
            result = (result * base) % m;
        }
        exp >>= 1;
        base = (base * base) % m;
    }
    result as i64
}

/// Modular inverse of `value` modulo `|modulus|` using the extended Euclidean
/// algorithm.  Returns `None` if the inverse does not exist (i.e.
/// `gcd(value, |modulus|) != 1`).
///
/// The result is in the range `[0, |modulus| - 1]` (always non-negative).
/// Callers that need the result adjusted for a negative `modulus` must handle
/// the sign themselves.
///
/// `modulus` must not be zero.
pub(crate) fn modinv_bigint(value: &PyBigInt, modulus: &PyBigInt) -> Option<PyBigInt> {
    use num_traits::One;

    // Absolute value of modulus so the algorithm works on positive numbers.
    let m: PyBigInt = if *modulus < PyBigInt::from(0i64) {
        -modulus
    } else {
        modulus.clone()
    };

    // Reduce value modulo m so old_r starts non-negative.
    let v = ((value % &m) + &m) % &m;

    // Extended Euclidean algorithm (Knuth Vol. 2, §4.5.2 Algorithm X).
    let mut old_r = v;
    let mut r = m.clone();
    let mut old_s = PyBigInt::one();
    let mut s = PyBigInt::from(0i64);

    while r != PyBigInt::from(0i64) {
        let quotient = &old_r / &r;
        let tmp_r = old_r - &quotient * &r;
        old_r = r;
        r = tmp_r;
        let tmp_s = old_s - &quotient * &s;
        old_s = s;
        s = tmp_s;
    }

    // old_r is gcd(value, m).  Inverse exists only when gcd == 1.
    if old_r != PyBigInt::one() {
        return None;
    }

    // Normalise the Bézout coefficient to [0, m).
    let result = ((old_s % &m) + &m) % &m;
    Some(result)
}

/// Modular inverse of `value` modulo `|modulus|` for i64.  Returns `None` if
/// the inverse does not exist.  The result is in `[0, |modulus| - 1]`.
///
/// Callers must ensure `modulus != 0`.
pub(crate) fn modinv_i64(value: i64, modulus: i64) -> Option<i64> {
    let m = modulus.unsigned_abs() as i128;
    if m == 0 {
        return None;
    }
    // Reduce value modulo m so old_r starts non-negative.
    let v = ((value as i128 % m) + m) % m;
    let mut old_r = v;
    let mut r = m;
    let mut old_s: i128 = 1;
    let mut s: i128 = 0;

    while r != 0 {
        let quotient = old_r / r;
        let tmp_r = old_r - quotient * r;
        old_r = r;
        r = tmp_r;
        let tmp_s = old_s - quotient * s;
        old_s = s;
        s = tmp_s;
    }

    if old_r != 1 {
        return None;
    }

    // Normalise to [0, m).
    let result = ((old_s % m) + m) % m;
    Some(result as i64)
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

/// Format the exception chain that precedes `exc_val` as a prefix string to
/// prepend to the main traceback output.
///
/// Walks `__cause__` (when `__suppress_context__ == True`) or `__context__`
/// (when `__suppress_context__` is falsy) and collects the chain from innermost
/// (closest to `exc_val`) to outermost.  The chain is then reversed and printed
/// oldest-first, matching CPython's display order.
///
/// Each chained exception is formatted as just its `"ClassName: msg"` line
/// (no "Traceback (most recent call last):" header) because pyrust does not
/// yet track per-exception frame captures — only the final propagating
/// exception has frame info.  CPython does the same when the chained
/// exception's `__traceback__` is `None`.
///
/// Returns an empty string when there is no visible chain (no `__cause__` /
/// `__context__`, or the chain is suppressed via `raise X from None`).
pub(crate) fn format_exc_chain_prefix(exc_val: &Value) -> String {
    // Collect (exc_value, is_cause) pairs from innermost to outermost.
    let mut chain: Vec<(Value, bool)> = Vec::new();
    let mut seen: HashSet<*const ()> = HashSet::new();

    let mut current = exc_val.clone();
    loop {
        let ValueKind::PyInstance(inst) = current.kind() else {
            break;
        };
        let raw_ptr = Rc::as_ptr(inst) as *const ();
        if !seen.insert(raw_ptr) {
            break; // cycle guard
        }
        let borrow = inst.borrow();
        let suppress = borrow
            .attrs
            .get("__suppress_context__")
            .and_then(|v| match v.kind() {
                ValueKind::Bool(b) => Some(b),
                _ => None,
            })
            .unwrap_or(false);

        if suppress {
            // raise X from Y: display __cause__ (if not None)
            let cause = borrow.attrs.get("__cause__").cloned();
            drop(borrow);
            match cause {
                Some(c) if !matches!(c.kind(), ValueKind::None) => {
                    // Check the predecessor for cycles before pushing it.
                    if let ValueKind::PyInstance(next_inst) = c.kind() {
                        if seen.contains(&(Rc::as_ptr(next_inst) as *const ())) {
                            break;
                        }
                    }
                    chain.push((c.clone(), true));
                    current = c;
                }
                _ => break,
            }
        } else {
            // Implicit chaining: display __context__ (if not None)
            let context = borrow.attrs.get("__context__").cloned();
            drop(borrow);
            match context {
                Some(c) if !matches!(c.kind(), ValueKind::None) => {
                    // Check the predecessor for cycles before pushing it.
                    if let ValueKind::PyInstance(next_inst) = c.kind() {
                        if seen.contains(&(Rc::as_ptr(next_inst) as *const ())) {
                            break;
                        }
                    }
                    chain.push((c.clone(), false));
                    current = c;
                }
                _ => break,
            }
        }
    }

    if chain.is_empty() {
        return String::new();
    }

    // chain is innermost-first; reverse to print oldest first.
    chain.reverse();

    let mut out = String::new();
    for (exc, is_cause) in chain {
        out.push_str(&format_single_exc_line(&exc));
        out.push('\n');
        out.push('\n');
        if is_cause {
            out.push_str(
                "The above exception was the direct cause of the following exception:\n",
            );
        } else {
            out.push_str(
                "During handling of the above exception, another exception occurred:\n",
            );
        }
        out.push('\n');
    }
    out
}

/// Format a single exception value as `"ClassName: msg"` (or just `"ClassName"`
/// when the message is empty).  Used by `format_exc_chain_prefix`.
fn format_single_exc_line(value: &Value) -> String {
    match value.kind() {
        ValueKind::PyInstance(inst) => {
            let class_name = inst.borrow().class.borrow().name.clone();
            let msg = value.to_py_str();
            if msg.is_empty() {
                class_name
            } else {
                format!("{class_name}: {msg}")
            }
        }
        _ => format!("Uncaught exception: {}", value.repr()),
    }
}

/// Call `__del__` on `val` if it is a `PyInstance` with a `__del__` method
/// AND no other Python-visible binding to the same instance still exists.
///
/// "Python-visible" bindings are:
///   - Named local-variable registers (indices `0..num_locals`).  Compiler
///     temporaries (index `>= num_locals`) are not Python names and must
///     not prevent `__del__` from firing.  The deleted register has already
///     been cleared to `Value::unset()` by the caller, so it naturally
///     produces no match during the scan.
///   - `interp.env.borrow().values`: global / nonlocal / cell-var bindings.
///
/// This deliberately ignores interpreter-internal state (reusable argument
/// buffers, inline caches, etc.) because those are implementation details
/// invisible to Python code, mirroring CPython's refcount semantics where
/// only Python-level references keep an object alive.
///
/// If `__del__` raises an exception, a CPython-format warning is printed to
/// stderr but the exception is not propagated to the caller (issue #1797).
pub(crate) fn call_del_if_last_binding(
    interp: &mut Interpreter,
    val: Value,
    regs: &RegSlice,
    num_locals: usize,
) {
    // Uninitialised register slots (Value::unset()) are not Python values.
    if val.is_unset() {
        return;
    }
    let del_rc = match val.as_py_instance_rc() {
        Some(rc) => rc,
        None => return,
    };
    // Look up __del__ before the scan so we exit early for objects without it.
    let method = match lookup_class_attr(&del_rc.borrow().class, "__del__") {
        Some(m)
            if matches!(
                m.kind(),
                ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
            ) =>
        {
            m
        }
        _ => return,
    };
    // Scan named local-variable registers (0..num_locals) for another binding.
    // Registers >= num_locals are compiler temporaries — not Python variables.
    let scan_limit = num_locals.min(regs.len());
    for i in 0..scan_limit {
        // Skip uninitialised register slots — Value::unset() is not a Python value.
        let r = &regs[i];
        if r.is_unset() {
            continue;
        }
        if let Some(other_rc) = r.as_py_instance_rc() {
            if Rc::ptr_eq(other_rc, del_rc) {
                return; // another named local still holds the instance
            }
        }
    }
    // Scan env.values for a Python-level binding (globals / nonlocals / cells).
    for v in interp.env.borrow().values.values() {
        if let Some(other_rc) = v.as_py_instance_rc() {
            if Rc::ptr_eq(other_rc, del_rc) {
                return; // a global/nonlocal/cell var still holds the instance
            }
        }
    }
    // No other Python-visible binding — invoke __del__.
    let class_name = del_rc.borrow().class.borrow().name.clone();
    let instance = Value::py_instance(Rc::clone(del_rc));
    drop(val); // release our reference before calling __del__
    // CPython prints a warning to stderr but does not propagate __del__
    // exceptions to the caller (issue #1797).
    if let Err(e) = invoke_class_method(interp, method, instance, &[]) {
        eprintln!("Exception ignored in: <function {}.__del__>", class_name);
        // For a Raised instance, format as "ClassName: msg" (CPython parity)
        // using format_single_exc_line, which calls to_py_str() on the
        // instance — matching CPython's `ValueError: oops` output.
        // For other PyError variants, Display already formats as "ClassName: msg".
        match &e {
            PyError::Raised(v) => eprintln!("{}", format_single_exc_line(v)),
            _ => eprintln!("{}", e),
        }
    }
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


