// `collections` module — body for the `collections` entry in
// `pyrust_builtin_modules!`.
//
// `Counter` and `defaultdict` are both real Python classes (defined via
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
    NativeIterFrame, invoke_class_method, iter_values, lookup_class_attr,
};
use crate::value::{PyInstance, PyKey, Value, ValueKind, key_repr};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

pyrust_module! {
    class Counter {
        /// CPython: Counter([iterable_or_mapping]) — tally elements.
        /// <https://docs.python.org/3/library/collections.html#collections.Counter>
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() > 1 {
                return Err(PyError::Runtime(format!(
                    "{FN_NAME}() takes at most one argument",
                )));
            }
            let mut counts: IndexMap<PyKey, Value> = IndexMap::new();
            if let Some(arg) = user.first() {
                if let ValueKind::Dict(map) = arg.value.kind() {
                    for (k, v) in map.iter() {
                        let count = match v.kind() {
                            ValueKind::Int(n) => n,
                            ValueKind::Bool(b) => b as i64,
                            _ => return Err(PyError::named(
                                "TypeError",
                                format!("{FN_NAME}() mapping values must be integers"),
                            )),
                        };
                        counts.insert(k.clone(), Value::int(count));
                    }
                } else {
                    for v in _interp.collect_iterable(arg.value.clone())? {
                        let key = v.to_key().ok_or_else(|| {
                            PyError::named(
                                "TypeError",
                                format!("{FN_NAME}() elements must be hashable"),
                            )
                        })?;
                        let next = match counts.get(&key).map(|v| v.kind()) {
                            Some(ValueKind::Int(n)) => n + 1,
                            _ => 1,
                        };
                        counts.insert(key, Value::int(next));
                    }
                }
            }
            inst.borrow_mut()
                .attrs
                .insert("_counts".to_string(), Value::dict(counts));
            Ok(Value::none())
        }

        /// Missing-key returns `0` — the dict-subclass quirk that makes
        /// Counter Counter.  This is the *only* defaulting branch; for
        /// proper present-key lookup we fall through to the stored map.
        fn __getitem__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let key = require_key(args, 1, FN_NAME)?;
            Ok(counts.get(&key).cloned().unwrap_or_else(|| Value::int(0)))
        }

        /// `c[k] = v` — Counter values must be integers (matches CPython).
        fn __setitem__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::Runtime(format!(
                    "{FN_NAME}() takes exactly 2 arguments",
                )));
            }
            let key = require_key(args, 1, FN_NAME)?;
            let value = match args[2].value.kind() {
                ValueKind::Int(_) | ValueKind::Bool(_) => args[2].value.clone(),
                _ => return Err(PyError::named(
                    "TypeError",
                    "Counter values must be integers".to_string(),
                )),
            };
            let mut counts = read_counts(args, FN_NAME)?;
            counts.insert(key, value);
            store_counts(&inst, counts);
            Ok(Value::none())
        }

        /// `key in c` — fall through to the stored map's contains.
        fn __contains__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let key = require_key(args, 1, FN_NAME)?;
            Ok(Value::bool_(counts.contains_key(&key)))
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
            let counts = read_counts(args, FN_NAME)?;
            let items: Vec<Value> = counts.keys().cloned().map(key_to_value).collect();
            Ok(Value::generator(Box::new(NativeIterFrame { items, pos: 0 })))
        }

        /// `repr(c)` — most-common-first, matching CPython.
        fn __repr__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            if counts.is_empty() {
                return Ok(Value::string("Counter()".to_string()));
            }
            let mut pairs: Vec<(PyKey, i64)> = counts
                .iter()
                .map(|(k, v)| (k.clone(), value_as_count(v)))
                .collect();
            pairs.sort_by(|a, b| b.1.cmp(&a.1));
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {v}", key_repr(k)))
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
            let mut pairs: Vec<(PyKey, i64)> = counts
                .iter()
                .map(|(k, v)| (k.clone(), value_as_count(v)))
                .collect();
            pairs.sort_by(|a, b| b.1.cmp(&a.1));
            let upper = n.unwrap_or(pairs.len()).min(pairs.len());
            Ok(Value::list(
                pairs
                    .into_iter()
                    .take(upper)
                    .map(|(k, v)| Value::tuple(vec![key_to_value(k), Value::int(v)]))
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

        /// `c.update(iterable_or_mapping)` — add to counts (mapping form
        /// uses values as deltas; iterable form adds 1 per element).
        fn update(args) -> Result<Value> {
            apply_delta(_interp, args, FN_NAME, /* sign = */ 1)
        }

        /// `c.subtract(iterable_or_mapping)` — subtract counts; the
        /// result can go below zero (`elements()` then skips them).
        fn subtract(args) -> Result<Value> {
            apply_delta(_interp, args, FN_NAME, /* sign = */ -1)
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
            // independent `_counts` payload.  Cheaper than going through
            // `Counter.__init__` again (which would re-tally from
            // scratch) — `c.copy()` is one of the hot paths.
            let inst = expect_self(args, FN_NAME)?;
            let class = Rc::clone(&inst.borrow().class);
            let mut attrs: IndexMap<String, Value> = IndexMap::new();
            attrs.insert("_counts".to_string(), Value::dict(counts));
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
            let key = require_key(args, 1, FN_NAME)?;
            match counts.get(&key) {
                Some(v) => Ok(v.clone()),
                None => Ok(user.get(1).cloned().map(|a| a.value).unwrap_or_else(Value::none)),
            }
        }

        /// `c.keys()` — list of keys (eager — see module docs).
        fn keys(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            require_no_args(args, "keys")?;
            Ok(Value::list(
                counts.keys().cloned().map(key_to_value).collect(),
            ))
        }

        /// `c.values()` — list of counts.
        fn values(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            require_no_args(args, "values")?;
            Ok(Value::list(counts.values().cloned().collect()))
        }

        /// `c.items()` — list of (key, count) pairs.
        fn items(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            require_no_args(args, "items")?;
            Ok(Value::list(
                counts
                    .iter()
                    .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                    .collect(),
            ))
        }

        /// `c + d` — add counts element-wise over the union of keys,
        /// then drop entries whose result is ≤ 0.  `d` may be a Counter
        /// or a plain dict (matches CPython's "any mapping" acceptance);
        /// any other type yields `NotImplemented` so the binary-op
        /// dispatch falls through to `__radd__` / `TypeError`.
        fn __add__(args) -> Result<Value> {
            counter_binop(args, CounterOp::Add)
        }

        /// `c - d` — subtract counts (treat missing as 0), drop ≤ 0.
        fn __sub__(args) -> Result<Value> {
            counter_binop(args, CounterOp::Sub)
        }

        /// `c & d` — element-wise min over the union of keys (missing
        /// counts treated as 0), drop ≤ 0.  Multiset intersection.
        fn __and__(args) -> Result<Value> {
            counter_binop(args, CounterOp::And)
        }

        /// `c | d` — element-wise max over the union of keys (missing
        /// counts treated as 0), drop ≤ 0.  Multiset union.
        fn __or__(args) -> Result<Value> {
            counter_binop(args, CounterOp::Or)
        }

        /// `c += d` — mutate `self._counts` in place and return `self`,
        /// preserving identity (CPython's augmented-op semantics).
        /// Non-Counter / non-dict RHS yields `NotImplemented` so the
        /// VM's in-place dispatch retries with plain `__add__`, which
        /// also returns `NotImplemented` and ultimately raises
        /// `TypeError`.
        fn __iadd__(args) -> Result<Value> {
            counter_inplace_op(args, CounterOp::Add)
        }

        fn __isub__(args) -> Result<Value> {
            counter_inplace_op(args, CounterOp::Sub)
        }

        fn __iand__(args) -> Result<Value> {
            counter_inplace_op(args, CounterOp::And)
        }

        fn __ior__(args) -> Result<Value> {
            counter_inplace_op(args, CounterOp::Or)
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
            if user.len() > 1 {
                return Err(PyError::Runtime(format!(
                    "{FN_NAME}() takes at most one argument",
                )));
            }
            let factory = user
                .first()
                .map(|a| a.value.clone())
                .unwrap_or_else(Value::none);
            if !factory.is_none() {
                let callable = matches!(
                    factory.kind(),
                    ValueKind::UserFunction(_)
                        | ValueKind::BuiltinFunction(_)
                        | ValueKind::BoundMethod { .. }
                        | ValueKind::ClassBoundMethod { .. }
                        | ValueKind::PyClass(_)
                );
                if !callable {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() first argument must be callable or None"),
                    ));
                }
            }
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("default_factory".to_string(), factory);
            attrs
                .attrs
                .insert("_items".to_string(), Value::dict(IndexMap::new()));
            Ok(Value::none())
        }

        /// Subscripted access — on miss, calls `__missing__` (which runs
        /// the factory) rather than raising KeyError directly.  Matches
        /// CPython's dict-subclass semantics where `defaultdict[k]` =
        /// `dict.__getitem__(self, k)` falls back to `self.__missing__(k)`.
        fn __getitem__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let key = require_key(args, 1, FN_NAME)?;
            let items = read_items(args, FN_NAME)?;
            if let Some(v) = items.get(&key) {
                return Ok(v.clone());
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
            Err(PyError::named("KeyError", args[1].value.repr()))
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
                return Err(PyError::named("KeyError", key_arg.value.repr()));
            }
            // Call the factory with no args.  The result is stored
            // under `key` and returned.
            let new_val = _interp.call_function_expanded(factory, &[])?;
            let pk = key_arg.value.to_key().ok_or_else(|| {
                PyError::named("TypeError", "unhashable type".to_string())
            })?;
            let mut items = read_items(args, FN_NAME)?;
            items.insert(pk, new_val.clone());
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
            let key = require_key(args, 1, FN_NAME)?;
            let mut items = read_items(args, FN_NAME)?;
            items.insert(key, args[2].value.clone());
            store_items(&inst, items);
            Ok(Value::none())
        }

        fn __contains__(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            let key = require_key(args, 1, FN_NAME)?;
            Ok(Value::bool_(items.contains_key(&key)))
        }

        fn __len__(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            Ok(Value::int(items.len() as i64))
        }

        fn __iter__(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            let keys: Vec<Value> = items.keys().cloned().map(key_to_value).collect();
            Ok(Value::generator(Box::new(NativeIterFrame {
                items: keys,
                pos: 0,
            })))
        }

        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let items = read_items(args, FN_NAME)?;
            let factory_repr = inst
                .borrow()
                .attrs
                .get("default_factory")
                .map(|v| v.repr())
                .unwrap_or_else(|| "None".to_string());
            let body: Vec<String> = items
                .iter()
                .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr()))
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
            let key = require_key(args, 1, FN_NAME)?;
            Ok(match items.get(&key) {
                Some(v) => v.clone(),
                None => user.get(1).cloned().map(|a| a.value).unwrap_or_else(Value::none),
            })
        }

        fn keys(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            require_no_args(args, "keys")?;
            Ok(Value::list(
                items.keys().cloned().map(key_to_value).collect(),
            ))
        }

        fn values(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            require_no_args(args, "values")?;
            Ok(Value::list(items.values().cloned().collect()))
        }

        fn items(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            require_no_args(args, "items")?;
            Ok(Value::list(
                items
                    .iter()
                    .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                    .collect(),
            ))
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
            let mut attrs: IndexMap<String, Value> = IndexMap::new();
            attrs.insert("default_factory".to_string(), factory);
            attrs.insert("_items".to_string(), Value::dict(items));
            Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class,
                attrs,
            }))))
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
        ValueKind::PyInstance(rc) => Ok(Rc::clone(&rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
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
) -> Result<IndexMap<PyKey, Value>> {
    let inst = expect_self(args, fn_name)?;
    let borrow = inst.borrow();
    match borrow.attrs.get("_counts") {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Ok(map.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}: Counter._counts has been overwritten with a non-dict; \
                     don't assign to internal attributes",
                ),
            )),
        },
        None => Ok(IndexMap::new()),
    }
}

/// Write `counts` back to `self._counts`.  Used by any method that
/// mutates the underlying tally (the `__init__` path inserts directly).
fn store_counts(inst: &Rc<RefCell<PyInstance>>, counts: IndexMap<PyKey, Value>) {
    inst.borrow_mut()
        .attrs
        .insert("_counts".to_string(), Value::dict(counts));
}

/// `defaultdict`'s storage accessor.  Same shape as `read_counts` but
/// against `self._items` (the dict slot defaultdict keeps the user's
/// data in — separate from `self.default_factory`).  TypeError on
/// external corruption, empty-map fallback when `__init__` hasn't run.
fn read_items(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<IndexMap<PyKey, Value>> {
    let inst = expect_self(args, fn_name)?;
    let borrow = inst.borrow();
    match borrow.attrs.get("_items") {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Ok(map.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}: defaultdict._items has been overwritten with a non-dict; \
                     don't assign to internal attributes",
                ),
            )),
        },
        None => Ok(IndexMap::new()),
    }
}

fn store_items(inst: &Rc<RefCell<PyInstance>>, items: IndexMap<PyKey, Value>) {
    inst.borrow_mut()
        .attrs
        .insert("_items".to_string(), Value::dict(items));
}

/// Hashable-key extraction at index `i` with a uniform TypeError on
/// non-hashable input.
fn require_key(args: &[ExpandedCallArg], i: usize, fn_name: &str) -> Result<PyKey> {
    let v = args.get(i).ok_or_else(|| {
        PyError::Runtime(format!("internal: {fn_name}() missing arg {i}"))
    })?;
    v.value
        .to_key()
        .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))
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

/// Apply `update`/`subtract` semantics: for each element of `other`,
/// adjust the count by `sign`.  `other` can be an iterable (counts as
/// +1/-1 per element) or a mapping (uses the mapping's integer values).
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
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let mut counts = read_counts(args, fn_name)?;
    let other = &user[0].value;

    if let ValueKind::Dict(map) = other.kind() {
        for (k, v) in map.iter() {
            let delta = match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    "Counter delta values must be integers".to_string(),
                )),
            };
            let cur = counts.get(k).map(value_as_count).unwrap_or(0);
            counts.insert(k.clone(), Value::int(cur + sign * delta));
        }
    } else {
        // Iterable form — each element contributes ±1.
        for v in interp.collect_iterable(other.clone())? {
            let key = v.to_key().ok_or_else(|| {
                PyError::named("TypeError", "unhashable type".to_string())
            })?;
            let cur = counts.get(&key).map(value_as_count).unwrap_or(0);
            counts.insert(key, Value::int(cur + sign));
        }
    }
    store_counts(&inst, counts);
    Ok(Value::none())
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
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => Value::string(s),
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::FrozenSet(items) => {
            pyrust_builtins::frozenset::frozenset(items.into_iter().collect())
        }
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
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
fn counts_of(other: &Value) -> Option<IndexMap<PyKey, Value>> {
    let ValueKind::PyInstance(inst) = other.kind() else {
        return None;
    };
    let borrow = inst.borrow();
    if borrow.class.borrow().name != "Counter" {
        return None;
    }
    match borrow.attrs.get("_counts") {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Some(map.clone()),
            _ => Some(IndexMap::new()),
        },
        None => Some(IndexMap::new()),
    }
}

/// Merge `lhs` and `rhs` per `op`, then drop entries whose result is
/// ≤ 0.  Shared core of all four binary ops; in-place variants write
/// the result back to `self._counts` while the regular `__add__`/etc.
/// return a fresh Counter.
fn merge_counts(
    lhs: &IndexMap<PyKey, Value>,
    rhs: &IndexMap<PyKey, Value>,
    op: CounterOp,
) -> IndexMap<PyKey, Value> {
    let mut out: IndexMap<PyKey, Value> = IndexMap::new();
    // Walk LHS first so the output preserves LHS insertion order for
    // shared keys — matches CPython, where `(c + d).keys()` lists
    // c-only and shared keys in c's order, then d-only keys.
    for (k, v) in lhs.iter() {
        let a = value_as_count(v);
        let b = rhs.get(k).map(value_as_count).unwrap_or(0);
        let result = op.apply(a, b);
        if result > 0 {
            out.insert(k.clone(), Value::int(result));
        }
    }
    for (k, v) in rhs.iter() {
        if lhs.contains_key(k) {
            continue;
        }
        let b = value_as_count(v);
        let result = op.apply(0, b);
        if result > 0 {
            out.insert(k.clone(), Value::int(result));
        }
    }
    out
}

/// Shared body for `__add__` / `__sub__` / `__and__` / `__or__`.
/// Returns a *new* Counter PyInstance with the merged counts.
fn counter_binop(args: &[ExpandedCallArg], op: CounterOp) -> Result<Value> {
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
    let merged = merge_counts(&lhs, &rhs, op);
    let class = Rc::clone(&inst.borrow().class);
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert("_counts".to_string(), Value::dict(merged));
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs,
    }))))
}

/// Shared body for `__iadd__` / `__isub__` / `__iand__` / `__ior__`.
/// Mutates `self._counts` and returns `self` (identity-preserving).
fn counter_inplace_op(args: &[ExpandedCallArg], op: CounterOp) -> Result<Value> {
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
    let merged = merge_counts(&lhs, &rhs, op);
    store_counts(&inst, merged);
    Ok(Value::py_instance(inst))
}
