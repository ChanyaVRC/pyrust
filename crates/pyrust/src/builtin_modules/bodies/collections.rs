// `collections` module — body for the `collections` entry in
// `pyrust_builtin_modules!`.
//
// `Counter`, `defaultdict`, and `deque` are real Python classes (defined via
// `pyrust_module!`'s `class { … }` block).  Their dunder methods
// (`__init__`, `__getitem__`, `__iter__`, `__missing__`, …) plug into
// pyrust's standard class-method dispatch, so iteration / subscript /
// `isinstance` work without per-type plumbing in the interpreter.
//
// State is stored as regular Python-level instance attributes:
//
// - **Counter**: `self._counts` (a dict).
// - **defaultdict**: `self.default_factory` (callable or None) +
//   `self._items` (a dict).
// - **deque**: `self._items` (a list, shared Rc<RefCell<Vec<Value>>>) +
//   `self.maxlen` (an int ≥ 0 or None for unbounded).  `maxlen` is stored
//   directly under its public name so `d.maxlen` resolves via the normal
//   `attrs` lookup without any `__getattr__` plumbing.
//
// This matches CPython's "Counter is a dict subclass" model in spirit —
// we don't currently subclass `dict` in pyrust, so the storage is a
// *named* dict attribute rather than the instance being a dict.
// `isinstance(c, dict)` is False as a result; if/when pyrust grows
// subclassing of built-in types, that flips on naturally.
//
// `defaultdict`'s missing-key path is the only place either class uses
// the new `__missing__` dunder: when `defaultdict.__getitem__` doesn't
// find the key, it calls `self.__missing__(key)`, which in turn runs
// the factory and stores the result.  CPython's exact mechanism.
//
// Reference: <https://docs.python.org/3/library/collections.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    GuardVersion, Interpreter, NativeIterFrame, NativeIterGuard, invoke_class_method,
    lookup_class_attr,
};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, Value, ValueKind, key_repr};
use pyrust_derive::pyrust_module;

/// Python-source definitions for `namedtuple`, `OrderedDict`, `ChainMap`,
/// `UserDict`, `UserList`, and `UserString` (issue #1884).  These members
/// are most naturally expressed in Python; they are exec'd once into a
/// throwaway namespace at first import of `collections` and the resulting
/// names copied onto the module.  See `inject_python_members`.
const COLLECTIONS_PY_SOURCE: &str = include_str!("collections_py.py");

/// Names defined by `COLLECTIONS_PY_SOURCE` that should be exported onto the
/// `collections` module.  Private helpers (`_keywords`, `_make_field_getter`,
/// `_sys_maxsize`) are intentionally omitted.
const COLLECTIONS_PY_EXPORTS: [&str; 6] = [
    "namedtuple",
    "OrderedDict",
    "ChainMap",
    "UserDict",
    "UserList",
    "UserString",
];

/// Execute `COLLECTIONS_PY_SOURCE` once and copy its public names onto the
/// `collections` module's attribute map.  Called from the `@inject` post-load
/// hook (`crate::builtin_modules::post_load_inject`) after the native classes
/// (`Counter`, `defaultdict`, `deque`) and `Counter`/`defaultdict`'s `dict`
/// re-parenting are in place, so the Python source can rely on the rest of
/// the module being present.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(COLLECTIONS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("collections: exec namespace not a dict".into()))?;
    for name in COLLECTIONS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    tag_public_classes(module);
    Ok(())
}

/// Tag each public `collections` class with `__module__ = "collections"` and a
/// `__class_getitem__` sentinel (issues #2228 / #2603).
///
/// `__module__` makes the type repr render `<class 'collections.Counter'>` and
/// `Counter.__module__ == "collections"`, matching CPython.  The native
/// classes (macro-built) carry no `__module__`; the Python-source classes are
/// exec'd in a private namespace and would otherwise pick up that namespace's
/// `__name__`.  Done after the exec above so every class exists.  `namedtuple`
/// is deliberately excluded — CPython gives namedtuple-created classes the
/// *caller's* `__module__`, not `collections`.
///
/// PEP 585 (issue #2603): every public `collections` container class defines
/// `__class_getitem__` in CPython 3.12, so `collections.OrderedDict[int]` etc.
/// produce a `types.GenericAlias`.  We register the same
/// `BuiltinFunction("<qualname>.__class_getitem__")` sentinel that
/// `build_primitive_classes` puts on `list`/`dict`; `eval_index`'s `PyClass`
/// arm detects the sentinel and builds the alias directly, while
/// `call_function_expanded` handles the explicit `Cls.__class_getitem__(int)`
/// call form.  The repr's `collections.` prefix comes from `__module__` set
/// just above plus the class's `qualname`.
fn tag_public_classes(module: &Rc<RefCell<crate::value::PyModule>>) {
    for cls_name in [
        "Counter",
        "defaultdict",
        "deque",
        "OrderedDict",
        "ChainMap",
        "UserDict",
        "UserList",
        "UserString",
    ] {
        let cls = module.borrow().attrs.get(cls_name).cloned();
        if let Some(cls_val) = cls
            && let ValueKind::PyClass(cls_rc) = cls_val.kind()
        {
            cls_rc
                .borrow_mut()
                .attrs
                .insert("__module__".to_string(), Value::string("collections"));
            let sentinel: &'static str =
                Box::leak(format!("{cls_name}.__class_getitem__").into_boxed_str());
            cls_rc.borrow_mut().attrs.insert(
                "__class_getitem__".to_string(),
                Value::builtin_function(sentinel),
            );
        }
    }
}

/// Attribute name under which `Counter` and `defaultdict` keep their backing
/// dict (issue #2010).  Both are real `dict` subclasses (their `PyClass.base`
/// is the `dict` singleton, set in `env.rs::load_module`), so the instance's
/// mapping must live under the same `__builtin_data__` key that the generic
/// dict-subclass machinery (`call_class_expanded`'s backing-store
/// pre-initialisation, `dict(instance)` conversion, subscript fallback) reads
/// and writes.  Storing the map here — rather than a private `_counts` /
/// `_items` attr — keeps the subclass coherent end-to-end.
const COUNTER_BACKING: &str = "__builtin_data__";

pyrust_module! {
    constants {
        // Expose the `collections.abc` submodule as `collections.abc` so that
        // `import collections; collections.abc` resolves correctly.  The
        // `load_module` parent-package identity fix-up in `env.rs` replaces
        // this with the cached submodule value on first import, ensuring
        // `collections.abc is collections.abc` identity holds.
        "abc" => super::collections_abc::module()
    }

    class Counter {
        /// CPython: Counter([iterable_or_mapping], **kwds) — tally elements.
        /// A positional iterable/mapping is tallied first, then any keyword
        /// arguments are *added* on top as string-keyed counts (#2013).
        /// <https://docs.python.org/3/library/collections.html#collections.Counter>
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            let positional: Vec<&ExpandedCallArg> =
                user.iter().filter(|a| a.name.is_none()).collect();
            let kwargs: Vec<&ExpandedCallArg> =
                user.iter().filter(|a| a.name.is_some()).collect();
            if positional.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{FN_NAME}() takes at most one positional argument ({} given)",
                        positional.len(),
                    ),
                ));
            }
            let mut counts: PyDict = PyDict::default();
            if let Some(arg) = positional.first() {
                counter_tally_into(_interp, &mut counts, &arg.value, FN_NAME, 1)?;
            }
            // Keyword arguments become string-keyed counts, added on top
            // (CPython `Counter('ab', a=10)` → a:11, b:1).
            counter_apply_kwargs(_interp, &mut counts, &kwargs, 1)?;
            inst.borrow_mut()
                .attrs
                .insert(COUNTER_BACKING, Value::dict(counts));
            Ok(Value::none())
        }

        /// Missing-key returns `0` — the dict-subclass quirk that makes
        /// Counter Counter.  This is the *only* defaulting branch; for
        /// proper present-key lookup we fall through to the stored map.
        fn __getitem__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            // #1919: `__eq__`-aware lookup so equal user-keys hit.
            Ok(map_get_eq(_interp, &counts, &key)?.unwrap_or_else(|| Value::int(0)))
        }

        /// `c[k] = v` — store any value under the key (CPython does not
        /// enforce integer-only counts in `__setitem__`; it is merely
        /// conventional to store integers).
        fn __setitem__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::Runtime(format!(
                    "{FN_NAME}() takes exactly 2 arguments",
                )));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let value = args[2].value.clone();
            let mut counts = read_counts(args, FN_NAME)?;
            // #1919: `__eq__`-aware insert (overwrites the equal entry in place).
            map_insert_eq(_interp, &mut counts, key, value)?;
            store_counts(&inst, counts);
            Ok(Value::none())
        }

        /// `key in c` — fall through to the stored map's contains.
        fn __contains__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            // #1919: `__eq__`-aware membership.
            Ok(Value::bool_(map_contains_eq(_interp, &counts, &key)?))
        }

        /// `len(c)` — number of stored entries.
        fn __len__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            Ok(Value::int(counts.len() as i64))
        }

        /// `for k in c` — yield the keys in insertion order, matching
        /// dict iteration semantics.  Materialised through a fresh
        /// `NativeIterFrame` so re-iteration works (each `iter(c)`
        /// snapshot is independent) without round-tripping through the
        /// `iter()` builtin's dispatch.
        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let counts = read_counts(args, FN_NAME)?;
            let items: Vec<Value> = counts.keys().cloned().map(key_to_value).collect();
            Ok(make_guarded_dict_subclass_iter(inst, items))
        }

        /// `repr(c)` — most-common-first when all values are integers
        /// (matching CPython's `most_common()` sort); falls back to
        /// insertion order when any value is non-integer (matching
        /// CPython's `try: most_common() except TypeError: dict(self)`).
        fn __repr__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            if counts.is_empty() {
                return Ok(Value::string("Counter()"));
            }
            // Collect as (key, value) preserving the actual stored value so
            // non-integer entries repr correctly (issue #920).
            let mut pairs: Vec<(PyKey, Value)> = counts
                .into_iter()
                .collect();
            // Sort by count descending, mirroring CPython's
            // `try: most_common() except TypeError: dict(self)` fallback.
            // We sort when all counts are numeric (int, bool, float); user-defined
            // or non-orderable values fall back to insertion order.
            let all_numeric = pairs.iter().all(|(_, v)| {
                matches!(
                    v.kind(),
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::Float(_)
                )
            });
            if all_numeric {
                pairs.sort_by(|a, b| {
                    let to_f64 = |v: &Value| match v.kind() {
                        ValueKind::Float(f) => f,
                        ValueKind::Bool(b) => if b { 1.0 } else { 0.0 },
                        _ => value_as_count(v) as f64,
                    };
                    to_f64(&b.1).total_cmp(&to_f64(&a.1))
                });
            }
            // else: leave in insertion order (CPython fallback for non-orderable values)
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr_raw()))
                .collect();
            Ok(Value::string(format!(
                "Counter({{{}}})",
                inner.join(", ")
            )))
        }

        /// `c.most_common(n=None)` — list of (element, count) pairs in
        /// descending-count order.  `n=None` yields all entries.
        fn most_common(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    "most_common() takes at most 1 argument".to_string(),
                ));
            }
            let n: Option<usize> = match user.first().map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => None,
                Some(ValueKind::Int(v)) if v >= 0 => Some(v as usize),
                Some(ValueKind::Bool(b)) => Some(b as usize),
                _ => return Err(PyError::named(
                    "TypeError",
                    "most_common() n must be a non-negative integer or None".to_string(),
                )),
            };
            let mut pairs: Vec<(PyKey, Value)> = counts
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            pairs.sort_by_key(|p| std::cmp::Reverse(value_as_count(&p.1)));
            let upper = n.unwrap_or(pairs.len()).min(pairs.len());
            Ok(Value::list(
                pairs
                    .into_iter()
                    .take(upper)
                    .map(|(k, v)| Value::tuple(vec![key_to_value(k), v]))
                    .collect(),
            ))
        }

        /// `c.elements()` — yields each element `count` times, for
        /// elements whose count is `> 0`.  Returns a plain list for
        /// initial-landing simplicity.
        fn elements(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let user = &args[1..];
            if !user.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "elements() takes no arguments".to_string(),
                ));
            }
            let mut out: Vec<Value> = Vec::new();
            for (k, v) in counts.iter() {
                let count = value_as_count(v);
                if count > 0 {
                    for _ in 0..count {
                        out.push(key_to_value(k.clone()));
                    }
                }
            }
            Ok(Value::list(out))
        }

        /// `c.update([iterable_or_mapping], **kwds)` — add to counts (mapping
        /// form uses values as deltas; iterable form adds 1 per element; any
        /// keyword arguments are added on top as string-keyed counts, #2013).
        fn update(args) -> Result<Value> {
            apply_delta(_interp, args, FN_NAME, /* sign = */ 1)
        }

        /// `c.subtract([iterable_or_mapping], **kwds)` — subtract counts; the
        /// result can go below zero (`elements()` then skips them).  Keyword
        /// arguments are subtracted as string-keyed counts (#2013).
        fn subtract(args) -> Result<Value> {
            apply_delta(_interp, args, FN_NAME, /* sign = */ -1)
        }

        /// `c.total()` — sum of the counts (Python 3.10+).
        /// <https://docs.python.org/3/library/collections.html#collections.Counter.total>
        fn total(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            require_no_args(args, "total")?;
            let sum: i64 = counts.values().map(value_as_count).sum();
            Ok(Value::int(sum))
        }

        /// `c.copy()` — return a new Counter with the same counts.
        fn copy(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let user = &args[1..];
            if !user.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "copy() takes no arguments".to_string(),
                ));
            }
            // Construct a fresh instance with the receiver's class and an
            // independent backing payload.  Cheaper than going through
            // `Counter.__init__` again (which would re-tally from
            // scratch) — `c.copy()` is one of the hot paths.
            let inst = expect_self(args, FN_NAME)?;
            let class = Rc::clone(&inst.borrow().class);
            let mut attrs = InstanceAttrs::new();
            attrs.insert(COUNTER_BACKING, Value::dict(counts));
            Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class,
                attrs,
            }))))
        }

        /// `c.get(key, default=None)` — present-key lookup without the
        /// missing-key→0 default that `c[key]` applies.  Mirrors
        /// `dict.get`.
        fn get(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    "get() takes 1 or 2 arguments".to_string(),
                ));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            // #1919: `__eq__`-aware lookup.
            match map_get_eq(_interp, &counts, &key)? {
                Some(v) => Ok(v),
                None => Ok(user.get(1).cloned().map(|a| a.value).unwrap_or_else(Value::none)),
            }
        }

        // keys/values/items return LIVE dict views sharing the backing Rc
        // (issue #2447).  Counter is a plain `dict` subclass in CPython, so the
        // views are PLAIN `dict_keys` / `dict_values` / `dict_items` (NOT
        // odict-tagged) with the plain "dictionary changed size during
        // iteration" guard wording.  The eager `list` snapshots they replaced
        // were the wrong type, not live across `update`/`subtract`, and — being
        // plain lists — silently completed when the Counter changed size
        // mid-iteration.
        fn keys(args) -> Result<Value> {
            require_no_args(args, "keys")?;
            let backing = live_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "keys", false)
        }

        fn values(args) -> Result<Value> {
            require_no_args(args, "values")?;
            let backing = live_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "values", false)
        }

        fn items(args) -> Result<Value> {
            require_no_args(args, "items")?;
            let backing = live_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "items", false)
        }

        /// `c + d` — add counts element-wise over the union of keys,
        /// then drop entries whose result is ≤ 0.  `d` may be a Counter
        /// or a plain dict (matches CPython's "any mapping" acceptance);
        /// any other type yields `NotImplemented` so the binary-op
        /// dispatch falls through to `__radd__` / `TypeError`.
        fn __add__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::Add)
        }

        /// `c - d` — subtract counts (treat missing as 0), drop ≤ 0.
        fn __sub__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::Sub)
        }

        /// `c & d` — element-wise min over the union of keys (missing
        /// counts treated as 0), drop ≤ 0.  Multiset intersection.
        fn __and__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::And)
        }

        /// `c | d` — element-wise max over the union of keys (missing
        /// counts treated as 0), drop ≤ 0.  Multiset union.
        fn __or__(args) -> Result<Value> {
            counter_binop(_interp, args, CounterOp::Or)
        }

        /// `c += d` — mutate `self._counts` in place and return `self`,
        /// preserving identity (CPython's augmented-op semantics).
        /// Non-Counter / non-dict RHS yields `NotImplemented` so the
        /// VM's in-place dispatch retries with plain `__add__`, which
        /// also returns `NotImplemented` and ultimately raises
        /// `TypeError`.
        fn __iadd__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::Add)
        }

        fn __isub__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::Sub)
        }

        fn __iand__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::And)
        }

        fn __ior__(args) -> Result<Value> {
            counter_inplace_op(_interp, args, CounterOp::Or)
        }
    }

    class defaultdict {
        /// CPython: defaultdict([default_factory[, ...]]).
        /// Stores `self.default_factory` and an empty `self._items` map.
        /// The factory is callable-checked at construction so users get
        /// the failure at the right line rather than on first missing
        /// access.
        /// <https://docs.python.org/3/library/collections.html#collections.defaultdict>
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            // CPython: `defaultdict(default_factory=None, /, *args, **kwargs)`.
            // The first *positional* arg is the factory; remaining positionals
            // and all keyword args initialise the dict exactly like `dict(...)`
            // (#2099).
            let positional: Vec<&ExpandedCallArg> =
                user.iter().filter(|a| a.name.is_none()).collect();
            let kwargs: Vec<&ExpandedCallArg> =
                user.iter().filter(|a| a.name.is_some()).collect();
            let factory = positional
                .first()
                .map(|a| a.value.clone())
                .unwrap_or_else(Value::none);
            if !factory.is_none()
                && !value_is_callable(&factory) {
                    return Err(PyError::named(
                        "TypeError",
                        "first argument must be callable or None".to_string(),
                    ));
                }
            // Everything after the factory is forwarded to dict init. CPython
            // allows at most one such positional (the dict initialiser).
            let dict_positionals = &positional[positional.len().min(1)..];
            if dict_positionals.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "dict expected at most 1 argument, got {}",
                        dict_positionals.len()
                    ),
                ));
            }
            let mut items = PyDict::default();
            dict_init_into(
                _interp,
                &mut items,
                dict_positionals.first().map(|a| &a.value),
                &kwargs,
            )?;
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("default_factory", factory);
            attrs
                .attrs
                .insert(COUNTER_BACKING, Value::dict(items));
            Ok(Value::none())
        }

        /// Subscripted access — on miss, calls `__missing__` (which runs
        /// the factory) rather than raising KeyError directly.  Matches
        /// CPython's dict-subclass semantics where `defaultdict[k]` =
        /// `dict.__getitem__(self, k)` falls back to `self.__missing__(k)`.
        fn __getitem__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let items = read_items(args, FN_NAME)?;
            // #1919: `__eq__`-aware lookup so equal user-keys hit.
            if let Some(v) = map_get_eq(_interp, &items, &key)? {
                return Ok(v);
            }
            // Miss → __missing__.  Resolved via the class so that
            // user-defined subclasses (when pyrust grows them) can
            // override.
            let class = Rc::clone(&inst.borrow().class);
            if let Some(missing) = lookup_class_attr(&class, "__missing__") {
                return invoke_class_method(
                    _interp,
                    missing,
                    Value::py_instance(inst),
                    &[args[1].clone()],
                );
            }
            Err(PyError::key_error(args[1].value.clone()))
        }

        /// `__missing__(key)` — call the factory (if non-None), store
        /// the result, return it.  `default_factory=None` falls through
        /// to a plain `KeyError`, matching CPython.
        fn __missing__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let key_arg = args.get(1).cloned().ok_or_else(|| {
                PyError::Runtime(format!("internal: {FN_NAME}() missing key arg"))
            })?;
            let factory = inst
                .borrow()
                .attrs
                .get("default_factory")
                .cloned()
                .unwrap_or_else(Value::none);
            if factory.is_none() {
                return Err(PyError::key_error(key_arg.value.clone()));
            }
            // Call the factory with no args.  The result is stored
            // under `key` and returned.
            let new_val = _interp.call_function_expanded(factory, &[])?;
            let pk = _interp.value_to_pykey(&key_arg.value)?;
            let mut items = read_items(args, FN_NAME)?;
            // #1919: `__eq__`-aware insert (no duplicate equal user-key).
            map_insert_eq(_interp, &mut items, pk, new_val.clone())?;
            store_items(&inst, items);
            Ok(new_val)
        }

        /// `d[k] = v` — straight write-through to the inner dict.
        fn __setitem__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::Runtime(format!(
                    "{FN_NAME}() takes exactly 2 arguments",
                )));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let mut items = read_items(args, FN_NAME)?;
            // #1919: `__eq__`-aware insert (overwrites equal entry in place).
            map_insert_eq(_interp, &mut items, key, args[2].value.clone())?;
            store_items(&inst, items);
            Ok(Value::none())
        }

        fn __contains__(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            // #1919: `__eq__`-aware membership.
            Ok(Value::bool_(map_contains_eq(_interp, &items, &key)?))
        }

        fn __len__(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            Ok(Value::int(items.len() as i64))
        }

        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let items = read_items(args, FN_NAME)?;
            let keys: Vec<Value> = items.keys().cloned().map(key_to_value).collect();
            Ok(make_guarded_dict_subclass_iter(inst, keys))
        }

        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let items = read_items(args, FN_NAME)?;
            let factory_repr = inst
                .borrow()
                .attrs
                .get("default_factory")
                .map(|v| v.repr_raw())
                .unwrap_or_else(|| "None".to_string());
            let body: Vec<String> = items
                .iter()
                .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr_raw()))
                .collect();
            Ok(Value::string(format!(
                "defaultdict({factory_repr}, {{{}}})",
                body.join(", ")
            )))
        }

        fn get(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    "get() takes 1 or 2 arguments".to_string(),
                ));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            // #1919: `__eq__`-aware lookup.
            Ok(match map_get_eq(_interp, &items, &key)? {
                Some(v) => v,
                None => user.get(1).cloned().map(|a| a.value).unwrap_or_else(Value::none),
            })
        }

        // keys/values/items return LIVE dict views sharing the backing Rc.
        // #2436 made these live but tagged them `ordered=true`; that wording
        // ("OrderedDict mutated during iteration") never surfaced because the
        // stale-Rc replace in `store_items` detached the view before any guard
        // could fire.  defaultdict is a PLAIN `dict` subclass in CPython, so the
        // guard wording is the plain "dictionary changed size during iteration"
        // — `ordered=false` (issue #2447).
        fn keys(args) -> Result<Value> {
            require_no_args(args, "keys")?;
            let backing = live_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "keys", false)
        }

        fn values(args) -> Result<Value> {
            require_no_args(args, "values")?;
            let backing = live_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "values", false)
        }

        fn items(args) -> Result<Value> {
            require_no_args(args, "items")?;
            let backing = live_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "items", false)
        }

        fn copy(args) -> Result<Value> {
            require_no_args(args, "copy")?;
            let inst = expect_self(args, FN_NAME)?;
            let items = read_items(args, FN_NAME)?;
            let factory = inst
                .borrow()
                .attrs
                .get("default_factory")
                .cloned()
                .unwrap_or_else(Value::none);
            let class = Rc::clone(&inst.borrow().class);
            let mut attrs = InstanceAttrs::new();
            attrs.insert("default_factory", factory);
            attrs.insert(COUNTER_BACKING, Value::dict(items));
            Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class,
                attrs,
            }))))
        }
    }

    class deque {
        /// CPython: deque([iterable[, maxlen]]) — double-ended queue.
        ///
        /// State:
        ///   `self._items`  — a list (Rc-shared Vec<Value>) used as a deque.
        ///   `self.maxlen`  — an int (≥ 0) or None (unbounded).  Stored under
        ///                    the public name so `d.maxlen` resolves via the
        ///                    normal attrs lookup without `__getattr__` plumbing.
        ///
        /// `__init__` accepts `maxlen` as either a positional arg or a
        /// keyword arg (matching CPython's `deque([iterable[, maxlen]])`
        /// and `deque(maxlen=5)` call forms).
        ///
        /// <https://docs.python.org/3/library/collections.html#collections.deque>
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            // Separate positional and keyword args.  deque accepts:
            //   deque()
            //   deque(iterable)
            //   deque(iterable, maxlen)
            //   deque(maxlen=N)
            //   deque(iterable, maxlen=N)
            let mut pos_iterable: Option<Value> = None;
            let mut pos_maxlen: Option<Value> = None;
            let mut kw_maxlen: Option<Value> = None;
            for arg in user {
                match &arg.name {
                    None => {
                        if pos_iterable.is_none() {
                            pos_iterable = Some(arg.value.clone());
                        } else if pos_maxlen.is_none() {
                            pos_maxlen = Some(arg.value.clone());
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                format!("{FN_NAME}() takes at most 2 arguments"),
                            ));
                        }
                    }
                    Some(name) if name == "maxlen" => {
                        if kw_maxlen.is_some() {
                            return Err(PyError::named(
                                "TypeError",
                                format!("{FN_NAME}() got multiple values for 'maxlen'"),
                            ));
                        }
                        kw_maxlen = Some(arg.value.clone());
                    }
                    Some(name) if name == "iterable" => {
                        if pos_iterable.is_some() {
                            return Err(PyError::named(
                                "TypeError",
                                format!("{FN_NAME}() got multiple values for 'iterable'"),
                            ));
                        }
                        pos_iterable = Some(arg.value.clone());
                    }
                    Some(name) => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{FN_NAME}() got an unexpected keyword argument '{name}'"
                            ),
                        ));
                    }
                }
            }
            // Resolve maxlen: keyword arg overrides positional when both present.
            if pos_maxlen.is_some() && kw_maxlen.is_some() {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() got multiple values for argument 'maxlen'"),
                ));
            }
            let raw_maxlen = kw_maxlen.or(pos_maxlen);
            let maxlen: Option<i64> = match raw_maxlen.as_ref().map(|v| v.kind()) {
                None | Some(ValueKind::None) => None,
                Some(ValueKind::Int(n)) if n >= 0 => Some(n),
                Some(ValueKind::Bool(b)) => Some(b as i64),
                Some(ValueKind::Int(_)) => {
                    return Err(PyError::named(
                        "ValueError",
                        "maxlen must be non-negative".to_string(),
                    ));
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "an integer is required".to_string(),
                    ));
                }
            };
            // Initialise _items from iterable (if provided), then trim to maxlen.
            let mut deque_items: Vec<Value> = Vec::new();
            if let Some(iterable) = pos_iterable {
                deque_items = _interp.collect_iterable(&iterable)?;
            }
            // Apply maxlen: if we already exceed it, keep the rightmost maxlen elements.
            if let Some(ml) = maxlen {
                let ml = ml as usize;
                if deque_items.len() > ml {
                    let drop = deque_items.len() - ml;
                    deque_items.drain(..drop);
                }
            }
            // Store `maxlen` under the public name so `d.maxlen` resolves directly.
            let maxlen_val = match maxlen {
                Some(n) => Value::int(n),
                None => Value::none(),
            };
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("_items", Value::list(deque_items));
            attrs.attrs.insert("maxlen", maxlen_val);
            Ok(Value::none())
        }

        /// `d.append(x)` — add to the right end.  When maxlen is set and
        /// the deque is full, the leftmost element is dropped.
        fn append(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let x = arg.clone();
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                return Ok(Value::none()); // maxlen=0: discard all appends
            }
            let items_val = deque_items_val(&inst)?;
            if let Some(ml) = maxlen
                && items_val.list_len().unwrap_or(0) >= ml {
                    // Drop from left to make room.
                    items_val.list_pop_at(0)?;
                }
            items_val.list_push(x)?;
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `d.appendleft(x)` — add to the left end.  When maxlen is set
        /// and the deque is full, the rightmost element is dropped.
        fn appendleft(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let x = arg.clone();
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                return Ok(Value::none()); // maxlen=0: discard all appends
            }
            let items_val = deque_items_val(&inst)?;
            if let Some(ml) = maxlen {
                let cur_len = items_val.list_len().unwrap_or(0);
                if cur_len >= ml {
                    // Drop from right to make room.
                    items_val.list_pop_at(cur_len - 1)?;
                }
            }
            items_val.list_insert(0, x)?;
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `d.pop()` — remove and return from the right.  Raises
        /// `IndexError` if the deque is empty.
        fn pop(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            let n = items_val.list_len().unwrap_or(0);
            if n == 0 {
                return Err(PyError::named(
                    "IndexError",
                    "pop from an empty deque".to_string(),
                ));
            }
            let popped = items_val.list_pop_at(n - 1)?;
            deque_bump_state(&inst);
            Ok(popped)
        }

        /// `d.popleft()` — remove and return from the left.  Raises
        /// `IndexError` if the deque is empty.
        fn popleft(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            let n = items_val.list_len().unwrap_or(0);
            if n == 0 {
                return Err(PyError::named(
                    "IndexError",
                    "pop from an empty deque".to_string(),
                ));
            }
            let popped = items_val.list_pop_at(0)?;
            deque_bump_state(&inst);
            Ok(popped)
        }

        /// `d.extend(iterable)` — extend right from an iterable, applying
        /// maxlen trimming along the way (same as repeated `append`).
        fn extend(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let new_items = _interp.collect_iterable(arg)?;
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                return Ok(Value::none()); // maxlen=0: nothing to extend
            }
            let items_val = deque_items_val(&inst)?;
            for x in new_items {
                if let Some(ml) = maxlen
                    && items_val.list_len().unwrap_or(0) >= ml {
                        items_val.list_pop_at(0)?;
                    }
                items_val.list_push(x)?;
                deque_bump_state(&inst);
            }
            Ok(Value::none())
        }

        /// `d.extendleft(iterable)` — extend left from an iterable,
        /// prepending each element in turn (which reverses the iterable's
        /// order — matching CPython).  Maxlen trimming from the right.
        fn extendleft(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let new_items = _interp.collect_iterable(arg)?;
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                return Ok(Value::none()); // maxlen=0: nothing to extend
            }
            let items_val = deque_items_val(&inst)?;
            for x in new_items {
                let cur_len = items_val.list_len().unwrap_or(0);
                if let Some(ml) = maxlen
                    && cur_len >= ml {
                        // Trim from the right end before prepending.
                        items_val.list_pop_at(cur_len - 1)?;
                    }
                items_val.list_insert(0, x)?;
                deque_bump_state(&inst);
            }
            Ok(Value::none())
        }

        /// `d.rotate(n=1)` — rotate the deque n steps to the right.
        /// Negative n rotates left.  `rotate(1)` is equivalent to
        /// `appendleft(pop())`.
        fn rotate(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes at most 1 argument"),
                ));
            }
            let n: i64 = match user.first().map(|a| a.value.kind()) {
                None => 1,
                Some(ValueKind::Int(v)) => v,
                Some(ValueKind::Bool(b)) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "an integer is required".to_string(),
                    ));
                }
            };
            // Snapshot, rotate, and write back atomically.  This avoids the
            // per-op index-arithmetic bugs that arise from mutating the list
            // in place while tracking offsets.
            let mut items = deque_items_snapshot(&inst)?;
            let len = items.len();
            if len == 0 {
                return Ok(Value::none());
            }
            // CPython bumps `deque->state` on every rotate of a non-empty deque,
            // even when the net order is unchanged (n == 0 or a full cycle), so
            // a `rotate()` mid-iteration always raises (#1994).
            deque_bump_state(&inst);
            if n == 0 {
                return Ok(Value::none());
            }
            // Normalise to right-rotation steps in [0, len).
            let steps = ((n % len as i64) + len as i64) as usize % len;
            if steps != 0 {
                // rotate_right(steps) moves last `steps` elements to front.
                items.rotate_right(steps);
                let items_val = deque_items_val(&inst)?;
                items_val.list_clear()?;
                items_val.list_extend(items)?;
            }
            Ok(Value::none())
        }

        /// `d.clear()` — remove all elements.  maxlen is preserved.
        fn clear(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            items_val.list_clear()?;
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `d.copy()` — shallow copy.  Returns a new deque with the same
        /// elements and the same maxlen.
        fn copy(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items = deque_items_snapshot(&inst)?;
            let maxlen_val = inst
                .borrow()
                .attrs
                .get("maxlen")
                .cloned()
                .unwrap_or_else(Value::none);
            let class = Rc::clone(&inst.borrow().class);
            let mut attrs = InstanceAttrs::new();
            attrs.insert("_items", Value::list(items));
            attrs.insert("maxlen", maxlen_val);
            Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class,
                attrs,
            }))))
        }

        /// `d.count(x)` — count occurrences of `x` using `==` equality.
        fn count(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let target = arg.clone();
            let items = deque_items_snapshot(&inst)?;
            let mut n: i64 = 0;
            for v in &items {
                if _interp.values_user_eq(v, &target)? {
                    n += 1;
                }
            }
            Ok(Value::int(n))
        }

        /// `d.remove(x)` — remove the first occurrence of `x`.  Raises
        /// `ValueError` if not found.
        fn remove(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let target = arg.clone();
            let items = deque_items_snapshot(&inst)?;
            let mut found: Option<usize> = None;
            for (i, v) in items.iter().enumerate() {
                if _interp.values_user_eq(v, &target)? {
                    found = Some(i);
                    break;
                }
            }
            match found {
                Some(i) => {
                    let items_val = deque_items_val(&inst)?;
                    items_val.list_pop_at(i)?;
                    deque_bump_state(&inst);
                    Ok(Value::none())
                }
                None => Err(PyError::named(
                    "ValueError",
                    format!("{} is not in deque", target.repr_raw()),
                )),
            }
        }

        /// `d.reverse()` — reverse in place.
        fn reverse(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            items_val.list_reverse()?;
            Ok(Value::none())
        }

        /// `d.index(x[, start[, stop]])` — first index of `x` in
        /// `d[start:stop]`.  Raises `ValueError` if not found.
        fn index(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 3 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 to 3 arguments"),
                ));
            }
            let target = user[0].value.clone();
            let items = deque_items_snapshot(&inst)?;
            let len = items.len();
            let start = match user.get(1).map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => 0usize,
                Some(ValueKind::Int(n)) => {
                    if n < 0 {
                        (len as i64 + n).max(0) as usize
                    } else {
                        (n as usize).min(len)
                    }
                }
                Some(ValueKind::Bool(b)) => b as usize,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "an integer is required".to_string(),
                    ));
                }
            };
            let stop = match user.get(2).map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => len,
                Some(ValueKind::Int(n)) => {
                    if n < 0 {
                        (len as i64 + n).max(0) as usize
                    } else {
                        (n as usize).min(len)
                    }
                }
                Some(ValueKind::Bool(b)) => b as usize,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "an integer is required".to_string(),
                    ));
                }
            };
            for (i, item) in items.iter().enumerate().take(stop).skip(start) {
                if _interp.values_user_eq(item, &target)? {
                    return Ok(Value::int(i as i64));
                }
            }
            Err(PyError::named(
                "ValueError",
                format!("{} is not in deque", target.repr_raw()),
            ))
        }

        /// `d.insert(i, x)` — insert `x` at position `i`.  Raises
        /// `IndexError` if the deque is already at its maximum size.
        fn insert(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 2 arguments"),
                ));
            }
            let maxlen = deque_maxlen(&inst);
            let items_val = deque_items_val(&inst)?;
            let cur_len = items_val.list_len().unwrap_or(0);
            if let Some(ml) = maxlen
                && cur_len >= ml {
                    return Err(PyError::named(
                        "IndexError",
                        "deque already at its maximum size".to_string(),
                    ));
                }
            let i: i64 = match args[1].value.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "an integer is required".to_string(),
                    ));
                }
            };
            let x = args[2].value.clone();
            // Clamp index like CPython: negative clamps to 0, beyond end
            // clamps to len.
            let idx = if i < 0 {
                (cur_len as i64 + i).max(0) as usize
            } else {
                (i as usize).min(cur_len)
            };
            items_val.list_insert(idx, x)?;
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `len(d)` — number of elements.
        fn __len__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            Ok(Value::int(items_val.list_len().unwrap_or(0) as i64))
        }

        /// `d[i]` — element at index `i`.  Negative indices count from the
        /// right.  Raises `IndexError` if out of range.
        fn __getitem__(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let items = deque_items_snapshot(&inst)?;
            let len = items.len();
            let idx = deque_resolve_index(arg.kind(), len, FN_NAME)?;
            Ok(items[idx].clone())
        }

        /// `d[i] = x` — set element at index `i`.
        fn __setitem__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 2 arguments"),
                ));
            }
            let items_val = deque_items_val(&inst)?;
            let len = items_val.list_len().unwrap_or(0);
            let idx = deque_resolve_index(args[1].value.kind(), len, FN_NAME)?;
            let x = args[2].value.clone();
            // Replace: pop at idx and insert x.  The list API doesn't have
            // a direct set-at, so we do pop + insert.
            items_val.list_pop_at(idx)?;
            items_val.list_insert(idx, x)?;
            Ok(Value::none())
        }

        /// `del d[i]` — delete element at index `i`.
        fn __delitem__(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            let len = items_val.list_len().unwrap_or(0);
            let idx = deque_resolve_index(arg.kind(), len, FN_NAME)?;
            items_val.list_pop_at(idx)?;
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `x in d` — membership test using `==` equality.
        fn __contains__(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let target = arg.clone();
            let items = deque_items_snapshot(&inst)?;
            for v in &items {
                if _interp.values_user_eq(v, &target)? {
                    return Ok(Value::bool_(true));
                }
            }
            Ok(Value::bool_(false))
        }

        /// `for x in d` — yield elements in left-to-right order.
        ///
        /// The iterator snapshots the elements but holds the live backing
        /// `_items` list and its length, so each `__next__` re-checks the
        /// length and raises `RuntimeError: deque mutated during iteration`
        /// when the deque's size changes mid-iteration (#1994), matching
        /// CPython.
        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let items = deque_items_snapshot(&inst)?;
            // Cache a raw pointer to element 0 of the `_state` cell (a fixed
            // one-element list) so the per-step guard reads the counter with a
            // single tagged-int load — no attribute lookup, decode, or RefCell
            // borrow — keeping deque iteration perf-neutral.  The `cell` Value
            // stored in the guard keeps the backing buffer alive.
            let cell = deque_state_cell(&inst);
            let version = deque_state(&inst);
            let counter: *const Value = cell
                .as_list()
                .and_then(|s| s.first())
                .map(|v| v as *const Value)
                .ok_or_else(|| PyError::named("RuntimeError", "deque _state cell missing".to_string()))?;
            let mut frame = NativeIterFrame::new(items, "generator");
            frame.guard = Some(Box::new(NativeIterGuard {
                container: cell,
                version,
                kind: GuardVersion::DequeState { counter },
                msg: "deque mutated during iteration",
                exhaust_first: false,
                od_seq: 0,
            }));
            Ok(Value::generator(Box::new(frame)))
        }

        /// `repr(d)` — `deque([1, 2, 3])` or `deque([1, 2, 3], maxlen=5)`.
        ///
        /// Each element's repr goes through the interpreter so that
        /// user-defined `__repr__` methods (and nested deques) render
        /// correctly, matching CPython's behaviour.
        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let items = deque_items_snapshot(&inst)?;
            let mut inner: Vec<String> = Vec::with_capacity(items.len());
            for v in &items {
                let r = match v.kind() {
                    ValueKind::PyInstance(inst_rc) => {
                        let inst_rc = Rc::clone(inst_rc);
                        let class = Rc::clone(&inst_rc.borrow().class);
                        if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
                            let result = invoke_class_method(
                                _interp,
                                method_val,
                                Value::py_instance(Rc::clone(&inst_rc)),
                                &[],
                            )?;
                            match result.kind() {
                                ValueKind::Str(s) => s.to_string(),
                                _ => v.repr_raw(),
                            }
                        } else {
                            v.repr_raw()
                        }
                    }
                    _ => v.repr_raw(),
                };
                inner.push(r);
            }
            let items_repr = format!("[{}]", inner.join(", "));
            let maxlen = deque_maxlen(&inst);
            let s = match maxlen {
                None => format!("deque({items_repr})"),
                Some(ml) => format!("deque({items_repr}, maxlen={ml})"),
            };
            Ok(Value::string(s))
        }

        /// `__setattr__` — CPython's deque is a C extension type with no
        /// `__dict__`, so attribute assignment is blocked for *all* names.
        /// CPython uses two distinct error messages:
        ///   - `maxlen`: "attribute 'maxlen' of 'collections.deque' objects is not writable"
        ///   - anything else: "'collections.deque' object has no attribute '<name>'"
        /// Internal attrs (`_items`, `maxlen`) are only written by `__init__`
        /// and `copy`, which bypass `__setattr__` via direct `attrs.insert`.
        fn __setattr__(args) -> Result<Value> {
            let _inst = expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 2 arguments"),
                ));
            }
            // CPython accepts any `str` subclass as an attribute name (an
            // `isinstance` relationship) and otherwise raises
            // `attribute name must be string, not '<type>'` — matching the
            // shared `attr_name_arg` validator the getattr/setattr/hasattr/
            // delattr builtins use (#2350).
            let attr_name = if crate::interpreter::is_str_or_str_subclass(&args[1].value) {
                crate::interpreter::extract_str_value(&args[1].value)
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "attribute name must be string, not '{}'",
                        crate::interpreter::value_type_name_str(&args[1].value)
                    ),
                ));
            };
            if attr_name == "maxlen" {
                return Err(PyError::named(
                    "AttributeError",
                    "attribute 'maxlen' of 'collections.deque' objects is not writable"
                        .to_string(),
                ));
            }
            Err(PyError::named(
                "AttributeError",
                format!("'collections.deque' object has no attribute '{attr_name}'"),
            ))
        }

        /// `d == other` — equal iff `other` is a deque with the same
        /// elements in the same order (element-wise `==`).  Non-deque
        /// comparisons return `NotImplemented`.
        fn __eq__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Ok(Value::not_implemented());
            }
            // Check that `other` is also a deque.
            let other = &args[1].value;
            let other_items = match deque_items_of(other) {
                Some(v) => v,
                None => return Ok(Value::not_implemented()),
            };
            let self_items = deque_items_snapshot(&inst)?;
            if self_items.len() != other_items.len() {
                return Ok(Value::bool_(false));
            }
            for (a, b) in self_items.iter().zip(other_items.iter()) {
                if !_interp.values_user_eq(a, b)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }

        /// `d + other` — concatenate two deques into a new deque (#2011).
        /// The result inherits `self`'s `maxlen` and is trimmed to it (keeping
        /// the rightmost elements), matching CPython.  A non-deque RHS raises
        /// `TypeError` directly (CPython's `deque.__add__` does not defer to
        /// `__radd__`).
        fn __add__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let other = &args[1].value;
            let other_items = match deque_items_of(other) {
                Some(v) => v,
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "can only concatenate deque (not \"{}\") to deque",
                            crate::interpreter::value_type_name_str(other),
                        ),
                    ));
                }
            };
            let mut items = deque_items_snapshot(&inst)?;
            items.extend(other_items);
            let maxlen = deque_maxlen(&inst);
            Ok(deque_from_items(&inst, items, maxlen))
        }

        /// `d * n` — repeat the deque `n` times into a new deque (#2011).
        /// `n <= 0` yields an empty deque.  The result inherits `self`'s
        /// `maxlen` and is trimmed to it (keeping rightmost), matching CPython.
        /// A non-int `n` raises `TypeError`.
        fn __mul__(args) -> Result<Value> {
            deque_repeat(_interp, args, FN_NAME)
        }

        /// `n * d` — reflected multiply, identical to `d * n` (#2011).
        fn __rmul__(args) -> Result<Value> {
            deque_repeat(_interp, args, FN_NAME)
        }
    }
}

// ── Counter helpers ──────────────────────────────────────────────────────────

/// Extract `self` (the first arg, conventionally) as a `PyInstance` Rc.
/// Every method declared inside `class Counter { … }` receives the
/// instance as `args[0]` by macro convention — this helper centralises
/// the downcast + error path so each method's first line stays clean.
fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    let first = args.first().ok_or_else(|| {
        PyError::Runtime(format!("internal: {fn_name}() called without self"))
    })?;
    match first.value.kind() {
        ValueKind::PyInstance(rc) => Ok(Rc::clone(rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Arity guard for a deque method that takes no positional arguments
/// (`pop`, `popleft`, `clear`, `copy`, `reverse`).  The argument-count
/// check runs *before* `expect_self`, so a wrong-arity call wins over a
/// bad-self call — matching the original open-coded order.
fn expect_self_no_args(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    expect_self(args, fn_name)
}

/// Arity guard for a deque method that takes exactly one positional
/// argument.  `expect_self` runs *before* the argument-count check, so a
/// bad-self call wins over a wrong-arity call — matching the original
/// open-coded order.  Returns both the receiver and a borrow of the lone
/// argument value.
fn expect_self_one_arg<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
) -> Result<(Rc<RefCell<PyInstance>>, &'a Value)> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    Ok((inst, &args[1].value))
}

/// Read the `_counts` dict off `self`.  Returns an empty IndexMap if the
/// attribute is missing (which means `__init__` hasn't run yet — should
/// only happen for direct PyInstance construction, but we tolerate it).
///
/// If `_counts` was overwritten externally (e.g. `c._counts = "lol"`),
/// returns a `TypeError` rather than the internal-error path — the
/// failure is *user-caused*, not an interpreter bug.
fn read_counts(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<PyDict> {
    let inst = expect_self(args, fn_name)?;
    let borrow = inst.borrow();
    match borrow.attrs.get(COUNTER_BACKING) {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Ok(map.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}: Counter backing store has been overwritten with a non-dict; \
                     don't assign to internal attributes",
                ),
            )),
        },
        None => Ok(PyDict::default()),
    }
}

/// Write `new_map` back to `self`'s `__builtin_data__` backing dict, mutating
/// the **existing** `Rc<RefCell<PyDict>>` in place when one is present rather
/// than wrapping a fresh `Rc` (issue #2447).
///
/// Live `keys()` / `values()` / `items()` views share the backing `Rc`.  The
/// old `Value::dict(new_map)` replace detached every such view on the next
/// mutation, so iteration over a view never observed the size change and the
/// `RuntimeError("dictionary changed size during iteration")` guard never
/// fired (CPython keeps the views live through `update` / `subtract` / `clear`
/// / `__setitem__`).  Overwriting the existing map in place keeps every live
/// view — and the size guard keyed on it — attached.
///
/// The fall-through arm only triggers before `__init__` has installed the
/// backing (or after a user overwrote it with a non-dict); both insert a fresh
/// dict, matching the previous behaviour.
fn store_backing(inst: &Rc<RefCell<PyInstance>>, new_map: PyDict) {
    let mut borrow = inst.borrow_mut();
    let mut new_map = Some(new_map);
    if let Some(v) = borrow.attrs.get(COUNTER_BACKING)
        && v.dict_with_mut(|m| *m = new_map.take().unwrap()).is_some()
    {
        return;
    }
    borrow
        .attrs
        .insert(COUNTER_BACKING, Value::dict(new_map.unwrap()));
}

/// Write `counts` back to `self`'s backing dict.  Used by any method that
/// mutates the underlying tally (the `__init__` path inserts directly).
fn store_counts(inst: &Rc<RefCell<PyInstance>>, counts: PyDict) {
    store_backing(inst, counts);
}

/// `defaultdict`'s storage accessor.  Same shape as `read_counts` — the
/// backing dict (`__builtin_data__`) holds the user's data, separate from
/// `self.default_factory`.  TypeError on external corruption, empty-map
/// fallback when `__init__` hasn't run.
/// The live `__builtin_data__` backing `Value` (Rc-shared dict) of an
/// OrderedDict/defaultdict instance — for view construction, which must NOT
/// clone the map (issue #2436).
fn live_backing(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut borrow = inst.borrow_mut();
    match borrow.attrs.get(COUNTER_BACKING) {
        Some(v) if matches!(v.kind(), ValueKind::Dict(_)) => Ok(v.clone()),
        Some(_) => Err(PyError::named(
            "TypeError",
            format!("{fn_name}: backing store has been overwritten with a non-dict"),
        )),
        // No backing yet (e.g. raw PyInstance, `__init__` not run): install one
        // so the view shares the same `Rc` a later `store_backing` mutates in
        // place (issue #2447) rather than dangling on a throwaway dict.
        None => {
            let backing = Value::dict(PyDict::default());
            borrow.attrs.insert(COUNTER_BACKING, backing.clone());
            Ok(backing)
        }
    }
}

fn read_items(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<PyDict> {
    let inst = expect_self(args, fn_name)?;
    let borrow = inst.borrow();
    match borrow.attrs.get(COUNTER_BACKING) {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Ok(map.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}: defaultdict backing store has been overwritten with a non-dict; \
                     don't assign to internal attributes",
                ),
            )),
        },
        None => Ok(PyDict::default()),
    }
}

fn store_items(inst: &Rc<RefCell<PyInstance>>, items: PyDict) {
    store_backing(inst, items);
}

/// Build a key iterator over `keys` for a `Counter` / `defaultdict` instance,
/// guarded against size mutation during iteration (#2201).  CPython raises
/// `RuntimeError("dictionary changed size during iteration")` when the dict
/// changes size mid-loop; value-only mutations (which preserve the key count)
/// are allowed.  The guard's `container` is the *instance* (not the backing
/// dict `Value`) so it re-resolves `__builtin_data__` via `live_collection_len`
/// each step.  This stays correct regardless of whether the backing `Rc` is
/// mutated in place (`store_backing`, #2447) or replaced.  `keys.len()` is the
/// backing dict's size at iterator creation (the keys are its live key set).
fn make_guarded_dict_subclass_iter(inst: Rc<RefCell<PyInstance>>, keys: Vec<Value>) -> Value {
    let recorded_len = keys.len() as i64;
    let mut frame = NativeIterFrame::new(keys, "generator");
    frame.guard = Some(Box::new(NativeIterGuard {
        container: Value::py_instance(inst),
        version: recorded_len,
        kind: GuardVersion::Size,
        msg: "dictionary changed size during iteration",
        exhaust_first: false,
        od_seq: 0,
    }));
    Value::generator(Box::new(frame))
}

/// Hashable-key extraction at index `i` with a uniform TypeError on
/// non-hashable input.  Uses the interpreter-aware hash path so that
/// slice keys (and any other type with a custom `__hash__`) are handled
/// correctly rather than falling back to the pure `Value::to_key()` path
/// which cannot hash slices (issue #905).
fn require_key(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    i: usize,
    fn_name: &str,
) -> Result<PyKey> {
    let v = args.get(i).ok_or_else(|| {
        PyError::Runtime(format!("internal: {fn_name}() missing arg {i}"))
    })?;
    interp.value_to_pykey(&v.value)
}

/// `__eq__`-aware `get` against an owned `_counts`/`_items` snapshot map
/// (issue #1919).  Routes `PyKey::Object` keys through the interpreter's
/// `dict_lookup_in` (the same `__hash__`-then-`__eq__` path the builtin dict
/// uses); primitive keys hit the raw `IndexMap::get` fast path inside
/// `dict_lookup_in`.  The snapshot is a local copy, so running user `__eq__`
/// against it cannot alias the live store.
fn map_get_eq(
    interp: &mut crate::Interpreter,
    map: &PyDict,
    key: &PyKey,
) -> Result<Option<Value>> {
    Ok(interp.dict_lookup_in(map, key)?.map(|(_, v)| v))
}

/// `__eq__`-aware `contains_key` against an owned snapshot map (issue #1919).
fn map_contains_eq(
    interp: &mut crate::Interpreter,
    map: &PyDict,
    key: &PyKey,
) -> Result<bool> {
    Ok(interp.dict_lookup_in(map, key)?.is_some())
}

/// `__eq__`-aware `insert` into an owned snapshot map (issue #1919): overwrites
/// an existing `__eq__`-equal entry in place rather than appending a duplicate
/// `PyKey::Object`.  Delegates to `Interpreter::dict_insert`.
fn map_insert_eq(
    interp: &mut crate::Interpreter,
    map: &mut PyDict,
    key: PyKey,
    value: Value,
) -> Result<()> {
    interp.dict_insert(map, key, value)
}

/// Mirror of the `callable()` builtin (builtins.rs): decide whether `v` may be
/// used as `defaultdict`'s `default_factory`.  CPython accepts *any* callable,
/// not just functions/types — a class with `__call__`, a bound method, or a
/// `functools.partial` are all valid factories.  Keep this in sync with the
/// `callable()` body; the earlier hand-rolled `matches!` here wrongly rejected
/// `__call__` instances and `partial` (#2099 review).
fn value_is_callable(v: &Value) -> bool {
    match v.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => {
            pyrust_builtins::bound_method::is_bound_method(v)
                || pyrust_builtins::super_bound_builtin::as_super_bound_builtin(v).is_some()
                || pyrust_builtins::property::property_partial_slot(v)
                    .is_some_and(|slot| slot.is_some())
                || pyrust_builtins::type_call_wrapper::as_type_call_wrapper(v).is_some()
        }
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__call__").is_some()
        }
        _ => false,
    }
}

/// Apply `dict.__init__`/`dict.update` semantics into `items`: an optional
/// positional mapping-or-iterable-of-pairs followed by string-keyed keyword
/// arguments (#2099 — `defaultdict(factory, mapping)` / `(factory, pairs)` /
/// `(factory, **kw)`).  Mirrors CPython: a mapping (anything with `keys()`) is
/// copied key/value; any other positional is iterated as length-2
/// `(key, value)` pairs.  Insertion is `__eq__`-aware via [`map_insert_eq`] so
/// equal user-keys dedup (#1919).
fn dict_init_into(
    interp: &mut crate::Interpreter,
    items: &mut PyDict,
    positional: Option<&Value>,
    kwargs: &[&ExpandedCallArg],
) -> Result<()> {
    if let Some(arg) = positional {
        // Mapping form: a plain dict is copied verbatim, matching CPython's
        // `dict(mapping)`.
        if let ValueKind::Dict(map) = arg.kind() {
            for (k, v) in map.iter() {
                map_insert_eq(interp, items, k.clone(), v.clone())?;
            }
        } else if let Some(pairs) = crate::interpreter::mapping_pairs_via_protocol(interp, arg)? {
            // Any `keys()`-bearing mapping (dict subclasses like Counter /
            // defaultdict / OrderedDict, ChainMap, UserDict, duck-typed user
            // mappings) — keyed via `keys()` + `__getitem__`, exactly like
            // `dict(mapping)`.
            for (k, v) in pairs {
                map_insert_eq(interp, items, k, v)?;
            }
        } else {
            // Iterable-of-pairs form: each element must be a length-2
            // sequence, unpacked into `(key, value)`.
            for (idx, elem) in interp.collect_iterable(arg)?.into_iter().enumerate() {
                let (k_val, v_val) = match elem.kind() {
                    ValueKind::List(els) => {
                        let len = els.len();
                        if len != 2 {
                            return Err(pyrust_core::value_err!(
                                "dictionary update sequence element #{idx} has length {len}; 2 is required"
                            ));
                        }
                        (els[0].clone(), els[1].clone())
                    }
                    ValueKind::Tuple(els) => {
                        let len = els.len();
                        if len != 2 {
                            return Err(pyrust_core::value_err!(
                                "dictionary update sequence element #{idx} has length {len}; 2 is required"
                            ));
                        }
                        (els[0].clone(), els[1].clone())
                    }
                    ValueKind::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let len = chars.len();
                        if len != 2 {
                            return Err(pyrust_core::value_err!(
                                "dictionary update sequence element #{idx} has length {len}; 2 is required"
                            ));
                        }
                        (
                            Value::string(chars[0].to_string()),
                            Value::string(chars[1].to_string()),
                        )
                    }
                    _ => {
                        return Err(pyrust_core::type_err!(
                            "cannot convert dictionary update sequence element #{idx} to a sequence"
                        ));
                    }
                };
                let pk = interp.value_to_pykey(&k_val)?;
                map_insert_eq(interp, items, pk, v_val)?;
            }
        }
    }
    // Keyword arguments overlay the positional data, matching CPython order.
    for kw in kwargs {
        let name = kw.name.as_deref().unwrap_or("");
        map_insert_eq(interp, items, PyKey::str_from(name), kw.value.clone())?;
    }
    Ok(())
}

/// Method-body convention: `keys()`, `values()`, `items()` etc. take no
/// args beyond `self`.  Centralised so the error message is uniform.
fn require_no_args(args: &[ExpandedCallArg], method: &str) -> Result<()> {
    if args.len() > 1 {
        Err(PyError::named(
            "TypeError",
            format!("{method}() takes no arguments"),
        ))
    } else {
        Ok(())
    }
}

/// Apply `update`/`subtract` semantics: tally an optional positional
/// iterable/mapping plus any keyword counts into `self._counts`, scaled by
/// `sign` (+1 for `update`, -1 for `subtract`).  At most one positional is
/// allowed; keyword arguments are string-keyed counts added on top (#2013).
///
/// Takes `interp` so user `__iter__` classes are honoured via
/// `collect_iterable` (issue #446).
fn apply_delta(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    sign: i64,
) -> Result<Value> {
    let user = &args[1..];
    let positional: Vec<&ExpandedCallArg> =
        user.iter().filter(|a| a.name.is_none()).collect();
    let kwargs: Vec<&ExpandedCallArg> =
        user.iter().filter(|a| a.name.is_some()).collect();
    if positional.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most one positional argument ({} given)",
                positional.len(),
            ),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let mut counts = read_counts(args, fn_name)?;
    if let Some(arg) = positional.first() {
        counter_tally_into(interp, &mut counts, &arg.value, fn_name, sign)?;
    }
    counter_apply_kwargs(interp, &mut counts, &kwargs, sign)?;
    store_counts(&inst, counts);
    Ok(Value::none())
}

/// Tally `other` into `counts`, scaled by `sign`.  `other` may be a plain
/// `dict` (values are integer deltas), a `Counter` (its stored counts are
/// deltas), or any other iterable (each element contributes `sign`).  Shared
/// by `Counter.__init__`, `update`, and `subtract` (#2013).
///
/// Value-type preservation (issue #930): CPython's `Counter.update(mapping)`
/// short-circuits to a plain `dict.update` (copying values verbatim, so a
/// `Bool` count stays `Bool`) when `self` is *empty*; otherwise every key is
/// re-added as an `int` (`count + self.get(key, 0)`).  We replicate that with
/// `preserve` = "add into an empty map with `sign == 1`", evaluated once
/// before the mapping is walked.
fn counter_tally_into(
    interp: &mut crate::Interpreter,
    counts: &mut PyDict,
    other: &Value,
    fn_name: &str,
    sign: i64,
) -> Result<()> {
    // CPython's `if self:` check — emptiness is sampled once, up front.
    let preserve = sign == 1 && counts.is_empty();
    if let ValueKind::Dict(map) = other.kind() {
        for (k, v) in map.iter() {
            let delta = match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() mapping values must be integers"),
                    ));
                }
            };
            if preserve {
                map_insert_eq(interp, counts, k.clone(), v.clone())?;
            } else {
                let cur = map_get_eq(interp, counts, k)?.map(|v| value_as_count(&v)).unwrap_or(0);
                map_insert_eq(interp, counts, k.clone(), Value::int(cur + sign * delta))?;
            }
        }
    } else if let Some(other_counts) = counts_of(other) {
        // `other` is a Counter — treat its stored counts as deltas, exactly
        // like the plain-dict branch (issue: `Counter.update(another_counter)`
        // was falling through to the iterable path).
        for (k, v) in other_counts.iter() {
            let delta = match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() mapping values must be integers"),
                    ));
                }
            };
            if preserve {
                map_insert_eq(interp, counts, k.clone(), v.clone())?;
            } else {
                let cur = map_get_eq(interp, counts, k)?.map(|v| value_as_count(&v)).unwrap_or(0);
                map_insert_eq(interp, counts, k.clone(), Value::int(cur + sign * delta))?;
            }
        }
    } else {
        // Iterable form — each element contributes ±1.
        for v in interp.collect_iterable(other)? {
            let key = interp.value_to_pykey(&v)?;
            let cur = map_get_eq(interp, counts, &key)?.map(|v| value_as_count(&v)).unwrap_or(0);
            map_insert_eq(interp, counts, key, Value::int(cur + sign))?;
        }
    }
    Ok(())
}

/// Add keyword-argument counts (string keys, integer deltas) into `counts`,
/// scaled by `sign`.  Shared by `Counter.__init__`/`update`/`subtract` (#2013).
///
/// Like [`counter_tally_into`], CPython routes kwargs through
/// `update(dict(kwds))`, so the same empty-`self` value-preservation rule
/// applies (`Counter(a=True)` → `{'a': True}`, issue #930).
fn counter_apply_kwargs(
    interp: &mut crate::Interpreter,
    counts: &mut PyDict,
    kwargs: &[&ExpandedCallArg],
    sign: i64,
) -> Result<()> {
    if kwargs.is_empty() {
        return Ok(());
    }
    let preserve = sign == 1 && counts.is_empty();
    for kw in kwargs {
        let name = kw.name.as_deref().unwrap_or("");
        let key = PyKey::str_from(name);
        if preserve {
            map_insert_eq(interp, counts, key, kw.value.clone())?;
        } else {
            let delta = value_as_count(&kw.value);
            let cur = map_get_eq(interp, counts, &key)?.map(|v| value_as_count(&v)).unwrap_or(0);
            map_insert_eq(interp, counts, key, Value::int(cur + sign * delta))?;
        }
    }
    Ok(())
}

fn value_as_count(v: &Value) -> i64 {
    match v.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => b as i64,
        _ => 0,
    }
}

fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => s,
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(items) => {
            pyrust_builtins::frozenset::frozenset(items.into_iter().collect())
        }
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Bytes(rc) => Value::bytes((*rc).clone()),
        PyKey::Complex(re, im) => Value::complex(re, im),
        PyKey::Object { value, .. } => value,
    }
}

// ── Counter arithmetic operators (issue #331) ────────────────────────────────
//
// CPython's Counter arithmetic operators (+, -, &, |) share the same
// shape: walk the union of keys between `self` and `other`, apply the
// per-key op (treating missing counts as 0), then drop any entry whose
// resulting count is ≤ 0.  This file factors that into one helper rather
// than four near-identical method bodies.
//
// `other` is accepted as either a Counter PyInstance (read via
// `_counts`) or a plain dict (matches CPython's "any mapping with int
// values" acceptance).  Any other type ends with `NotImplemented`,
// which the binary-op dispatch in `eval_binary` converts into a proper
// TypeError after also trying the reflected dunder.
//
// In-place variants reuse the same merge function, then write the
// result back to `self._counts` and return `self` — preserving object
// identity, the property `c += d` is supposed to guarantee.

/// Which Counter arithmetic op to perform.  Kept as a plain enum (rather
/// than a function pointer) so the merge loop can lean on the optimizer
/// to specialise the inner match per call site.
#[derive(Copy, Clone)]
enum CounterOp {
    Add,
    Sub,
    And,
    Or,
}

impl CounterOp {
    fn apply(self, a: i64, b: i64) -> i64 {
        match self {
            CounterOp::Add => a + b,
            CounterOp::Sub => a - b,
            CounterOp::And => a.min(b),
            CounterOp::Or => a.max(b),
        }
    }
}

/// Extract a `_counts`-equivalent map from `other`:
///
/// - Counter PyInstance → its `_counts` dict (cloned).
/// - Anything else → `Ok(None)`, so the caller returns `NotImplemented`
///   and the binary-op dispatch raises `TypeError`.
///
/// We intentionally **do not** accept plain `dict` on the RHS — CPython
/// rejects `Counter() + {...}` with `TypeError` for `+`, `-`, and `&`,
/// and only "accepts" `|` because Counter inherits `dict.__or__` (a
/// path we can't reproduce without dict subclassing).  Routing dict
/// through here would diverge from CPython parity.
fn counts_of(other: &Value) -> Option<PyDict> {
    let ValueKind::PyInstance(inst) = other.kind() else {
        return None;
    };
    let borrow = inst.borrow();
    if borrow.class.borrow().name != "Counter" {
        return None;
    }
    match borrow.attrs.get(COUNTER_BACKING) {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Some(map.clone()),
            _ => Some(PyDict::default()),
        },
        None => Some(PyDict::default()),
    }
}

/// Merge `lhs` and `rhs` per `op`, then drop entries whose result is
/// ≤ 0.  Shared core of all four binary ops; in-place variants write
/// the result back to `self._counts` while the regular `__add__`/etc.
/// return a fresh Counter.
fn merge_counts(
    interp: &mut crate::Interpreter,
    lhs: &PyDict,
    rhs: &PyDict,
    op: CounterOp,
) -> Result<PyDict> {
    let mut out: PyDict = PyDict::default();
    // Walk LHS first so the output preserves LHS insertion order for
    // shared keys — matches CPython, where `(c + d).keys()` lists
    // c-only and shared keys in c's order, then d-only keys.
    for (k, v) in lhs.iter() {
        let a = value_as_count(v);
        // #1919: `__eq__`-aware lookup so an equal user-key in `rhs` is found.
        let b = map_get_eq(interp, rhs, k)?.map(|v| value_as_count(&v)).unwrap_or(0);
        let result = op.apply(a, b);
        if result > 0 {
            out.insert(k.clone(), Value::int(result));
        }
    }
    for (k, v) in rhs.iter() {
        // #1919: `__eq__`-aware membership against `lhs` to skip shared keys.
        if map_contains_eq(interp, lhs, k)? {
            continue;
        }
        let b = value_as_count(v);
        let result = op.apply(0, b);
        if result > 0 {
            out.insert(k.clone(), Value::int(result));
        }
    }
    Ok(out)
}

/// Shared body for `__add__` / `__sub__` / `__and__` / `__or__`.
/// Returns a *new* Counter PyInstance with the merged counts.
fn counter_binop(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    op: CounterOp,
) -> Result<Value> {
    let lhs = read_counts(args, "Counter.__binop__")?;
    let inst = expect_self(args, "Counter.__binop__")?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "Counter arithmetic op takes exactly 1 argument".to_string(),
        ));
    }
    let rhs = match counts_of(&user[0].value) {
        Some(m) => m,
        None => return Ok(Value::not_implemented()),
    };
    let merged = merge_counts(interp, &lhs, &rhs, op)?;
    let class = Rc::clone(&inst.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.insert(COUNTER_BACKING, Value::dict(merged));
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// Shared body for `__iadd__` / `__isub__` / `__iand__` / `__ior__`.
/// Mutates `self._counts` and returns `self` (identity-preserving).
fn counter_inplace_op(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    op: CounterOp,
) -> Result<Value> {
    let lhs = read_counts(args, "Counter.__inplace__")?;
    let inst = expect_self(args, "Counter.__inplace__")?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "Counter arithmetic op takes exactly 1 argument".to_string(),
        ));
    }
    let rhs = match counts_of(&user[0].value) {
        Some(m) => m,
        None => return Ok(Value::not_implemented()),
    };
    let merged = merge_counts(interp, &lhs, &rhs, op)?;
    store_counts(&inst, merged);
    Ok(Value::py_instance(inst))
}

// ── deque helpers ────────────────────────────────────────────────────────────

/// Return the deque's mutation-state cell (#1994): a one-element list `[counter]`
/// stored under `_state`, lazily created on first access.  Mirrors CPython's
/// `deque->state` — a version bumped on every structural mutation.  It is held
/// in a list (an `Rc`-shared cell) rather than a plain int attr so the iterator
/// can cache the `Rc` once and re-read the counter each `__next__` with a single
/// `Rc` deref + index, paying *no* per-step attribute lookup (keeps deque
/// iteration perf-neutral).
fn deque_state_cell(inst: &Rc<RefCell<PyInstance>>) -> Value {
    {
        let borrow = inst.borrow();
        if let Some(v) = borrow.attrs.get("_state")
            && v.is_list() {
                return v.clone();
            }
    }
    let cell = Value::list(vec![Value::int(0)]);
    inst.borrow_mut()
        .attrs
        .insert("_state", cell.clone());
    cell
}

/// Read the current deque mutation-state counter.
fn deque_state(inst: &Rc<RefCell<PyInstance>>) -> i64 {
    deque_state_cell(inst)
        .as_list()
        .and_then(|s| s.first())
        .map(|v| match v.kind() {
            ValueKind::Int(n) => n,
            _ => 0,
        })
        .unwrap_or(0)
}

/// Bump the deque's mutation-state counter (#1994).  Called by every structural
/// mutation so the iterator's snapshotted state diverges and `__next__` raises
/// `RuntimeError: deque mutated during iteration`.  Wraps on `i64` overflow
/// (benign — would require 2^63 mutations in a single iteration).
fn deque_bump_state(inst: &Rc<RefCell<PyInstance>>) {
    let cell = deque_state_cell(inst);
    let next = cell
        .as_list()
        .and_then(|s| s.first())
        .map(|v| match v.kind() {
            ValueKind::Int(n) => n,
            _ => 0,
        })
        .unwrap_or(0)
        .wrapping_add(1);
    // Replace element 0 in place (keeps the same backing Rc so iterators that
    // cached the cell observe the bump).
    cell.list_with_mut(|v| {
        if let Some(slot) = v.first_mut() {
            *slot = Value::int(next);
        }
    });
}

/// Return the `_items` list `Value` from a deque instance.  The Value holds
/// an `Rc<RefCell<Vec<Value>>>` so mutations are shared — callers can mutate
/// through the list API without writing back to attrs.
fn deque_items_val(inst: &Rc<RefCell<PyInstance>>) -> Result<Value> {
    let borrow = inst.borrow();
    match borrow.attrs.get("_items") {
        Some(v) if v.is_list() => Ok(v.clone()),
        Some(_) => Err(PyError::named(
            "TypeError",
            "deque._items has been overwritten with a non-list; \
             don't assign to internal attributes"
                .to_string(),
        )),
        None => Ok(Value::list(Vec::new())),
    }
}

/// Snapshot the deque's items as a `Vec<Value>` for read-only work.
fn deque_items_snapshot(inst: &Rc<RefCell<PyInstance>>) -> Result<Vec<Value>> {
    let items_val = deque_items_val(inst)?;
    Ok(items_val
        .as_list()
        .map(|s| s.to_vec())
        .unwrap_or_default())
}

/// Read the maxlen from `self.maxlen` — returns `None` for unbounded.
fn deque_maxlen(inst: &Rc<RefCell<PyInstance>>) -> Option<usize> {
    let borrow = inst.borrow();
    match borrow.attrs.get("maxlen").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) if n >= 0 => Some(n as usize),
        Some(ValueKind::Bool(b)) => Some(b as usize),
        _ => None,
    }
}

/// Extract the `_items` list snapshot from a Value that is a deque instance.
/// Returns `None` if the Value is not a deque `PyInstance`.  Used by `__eq__`.
fn deque_items_of(value: &Value) -> Option<Vec<Value>> {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return None;
    };
    let borrow = inst.borrow();
    if borrow.class.borrow().name != "deque" {
        return None;
    }
    match borrow.attrs.get("_items") {
        Some(v) => v.as_list().map(|s| s.to_vec()),
        None => Some(Vec::new()),
    }
}

/// Resolve a Python index (possibly negative) into a `usize` for a deque of
/// length `len`.  Raises `IndexError` if out of range.
fn deque_resolve_index(kind: ValueKind<'_>, len: usize, fn_name: &str) -> Result<usize> {
    let i: i64 = match kind {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}: indices must be integers"),
            ));
        }
    };
    let idx = if i < 0 {
        let adjusted = i + len as i64;
        if adjusted < 0 {
            return Err(PyError::named(
                "IndexError",
                "deque index out of range".to_string(),
            ));
        }
        adjusted as usize
    } else {
        i as usize
    };
    if idx >= len {
        return Err(PyError::named(
            "IndexError",
            "deque index out of range".to_string(),
        ));
    }
    Ok(idx)
}

/// Build a new deque `PyInstance` (same class as `proto`) holding `items`,
/// with the given `maxlen`.  If `maxlen` is set and `items` is longer, the
/// rightmost `maxlen` elements are kept — matching CPython's `+`/`*` result
/// trimming (#2011).
fn deque_from_items(
    proto: &Rc<RefCell<PyInstance>>,
    mut items: Vec<Value>,
    maxlen: Option<usize>,
) -> Value {
    if let Some(ml) = maxlen
        && items.len() > ml {
            let drop = items.len() - ml;
            items.drain(..drop);
        }
    let maxlen_val = match maxlen {
        Some(n) => Value::int(n as i64),
        None => Value::none(),
    };
    let class = Rc::clone(&proto.borrow().class);
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_items", Value::list(items));
    attrs.insert("maxlen", maxlen_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Shared body for deque `__mul__` / `__rmul__` (#2011).  Repeats the deque
/// `n` times into a new deque; `n <= 0` yields empty.  The result inherits
/// `self`'s maxlen (trimmed).  A non-int multiplier raises `TypeError`.
fn deque_repeat(
    _interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let n: i64 = match args[1].value.kind() {
        ValueKind::Int(v) => v,
        ValueKind::Bool(b) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "can't multiply sequence by non-int of type '{}'",
                    crate::interpreter::value_type_name_str(&args[1].value),
                ),
            ));
        }
    };
    let base = deque_items_snapshot(&inst)?;
    let reps = n.max(0) as usize;
    let mut items: Vec<Value> = Vec::with_capacity(base.len().saturating_mul(reps));
    for _ in 0..reps {
        items.extend(base.iter().cloned());
    }
    let maxlen = deque_maxlen(&inst);
    Ok(deque_from_items(&inst, items, maxlen))
}
