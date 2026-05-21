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
    NativeIterFrame, invoke_class_method, lookup_class_attr,
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
                        let key = _interp.value_to_pykey(&v)?;
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
            let key = require_key(_interp, args, 1, FN_NAME)?;
            Ok(counts.get(&key).cloned().unwrap_or_else(|| Value::int(0)))
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
            counts.insert(key, value);
            store_counts(&inst, counts);
            Ok(Value::none())
        }

        /// `key in c` — fall through to the stored map's contains.
        fn __contains__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
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

        /// `repr(c)` — most-common-first when all values are integers
        /// (matching CPython's `most_common()` sort); falls back to
        /// insertion order when any value is non-integer (matching
        /// CPython's `try: most_common() except TypeError: dict(self)`).
        fn __repr__(args) -> Result<Value> {
            let counts = read_counts(args, FN_NAME)?;
            if counts.is_empty() {
                return Ok(Value::string("Counter()".to_string()));
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
                .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr()))
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
            let key = require_key(_interp, args, 1, FN_NAME)?;
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
            let key = require_key(_interp, args, 1, FN_NAME)?;
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
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let mut items = read_items(args, FN_NAME)?;
            items.insert(key, args[2].value.clone());
            store_items(&inst, items);
            Ok(Value::none())
        }

        fn __contains__(args) -> Result<Value> {
            let items = read_items(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
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
            let key = require_key(_interp, args, 1, FN_NAME)?;
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
                deque_items = _interp.collect_iterable(iterable)?;
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
                Some(n) => Value::int(n as i64),
                None => Value::none(),
            };
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("_items".to_string(), Value::list(deque_items));
            attrs.attrs.insert("maxlen".to_string(), maxlen_val);
            Ok(Value::none())
        }

        /// `d.append(x)` — add to the right end.  When maxlen is set and
        /// the deque is full, the leftmost element is dropped.
        fn append(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let x = args[1].value.clone();
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                return Ok(Value::none()); // maxlen=0: discard all appends
            }
            let items_val = deque_items_val(&inst)?;
            if let Some(ml) = maxlen {
                if items_val.list_len().unwrap_or(0) >= ml {
                    // Drop from left to make room.
                    items_val.list_pop_at(0)?;
                }
            }
            items_val.list_push(x)?;
            Ok(Value::none())
        }

        /// `d.appendleft(x)` — add to the left end.  When maxlen is set
        /// and the deque is full, the rightmost element is dropped.
        fn appendleft(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let x = args[1].value.clone();
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
            Ok(Value::none())
        }

        /// `d.pop()` — remove and return from the right.  Raises
        /// `IndexError` if the deque is empty.
        fn pop(args) -> Result<Value> {
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let inst = expect_self(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            let n = items_val.list_len().unwrap_or(0);
            if n == 0 {
                return Err(PyError::named(
                    "IndexError",
                    "pop from an empty deque".to_string(),
                ));
            }
            Ok(items_val.list_pop_at(n - 1)?)
        }

        /// `d.popleft()` — remove and return from the left.  Raises
        /// `IndexError` if the deque is empty.
        fn popleft(args) -> Result<Value> {
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let inst = expect_self(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            let n = items_val.list_len().unwrap_or(0);
            if n == 0 {
                return Err(PyError::named(
                    "IndexError",
                    "pop from an empty deque".to_string(),
                ));
            }
            Ok(items_val.list_pop_at(0)?)
        }

        /// `d.extend(iterable)` — extend right from an iterable, applying
        /// maxlen trimming along the way (same as repeated `append`).
        fn extend(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let new_items = _interp.collect_iterable(args[1].value.clone())?;
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                return Ok(Value::none()); // maxlen=0: nothing to extend
            }
            let items_val = deque_items_val(&inst)?;
            for x in new_items {
                if let Some(ml) = maxlen {
                    if items_val.list_len().unwrap_or(0) >= ml {
                        items_val.list_pop_at(0)?;
                    }
                }
                items_val.list_push(x)?;
            }
            Ok(Value::none())
        }

        /// `d.extendleft(iterable)` — extend left from an iterable,
        /// prepending each element in turn (which reverses the iterable's
        /// order — matching CPython).  Maxlen trimming from the right.
        fn extendleft(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let new_items = _interp.collect_iterable(args[1].value.clone())?;
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                return Ok(Value::none()); // maxlen=0: nothing to extend
            }
            let items_val = deque_items_val(&inst)?;
            for x in new_items {
                let cur_len = items_val.list_len().unwrap_or(0);
                if let Some(ml) = maxlen {
                    if cur_len >= ml {
                        // Trim from the right end before prepending.
                        items_val.list_pop_at(cur_len - 1)?;
                    }
                }
                items_val.list_insert(0, x)?;
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
            if len == 0 || n == 0 {
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
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let inst = expect_self(args, FN_NAME)?;
            let items_val = deque_items_val(&inst)?;
            items_val.list_clear()?;
            Ok(Value::none())
        }

        /// `d.copy()` — shallow copy.  Returns a new deque with the same
        /// elements and the same maxlen.
        fn copy(args) -> Result<Value> {
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let inst = expect_self(args, FN_NAME)?;
            let items = deque_items_snapshot(&inst)?;
            let maxlen_val = inst
                .borrow()
                .attrs
                .get("maxlen")
                .cloned()
                .unwrap_or_else(Value::none);
            let class = Rc::clone(&inst.borrow().class);
            let mut attrs: IndexMap<String, Value> = IndexMap::new();
            attrs.insert("_items".to_string(), Value::list(items));
            attrs.insert("maxlen".to_string(), maxlen_val);
            Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class,
                attrs,
            }))))
        }

        /// `d.count(x)` — count occurrences of `x` using `==` equality.
        fn count(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let target = args[1].value.clone();
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
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let target = args[1].value.clone();
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
                    Ok(Value::none())
                }
                None => Err(PyError::named(
                    "ValueError",
                    format!("{} is not in deque", target.repr()),
                )),
            }
        }

        /// `d.reverse()` — reverse in place.
        fn reverse(args) -> Result<Value> {
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let inst = expect_self(args, FN_NAME)?;
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
            for i in start..stop {
                if _interp.values_user_eq(&items[i], &target)? {
                    return Ok(Value::int(i as i64));
                }
            }
            Err(PyError::named(
                "ValueError",
                format!("{} is not in deque", target.repr()),
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
            if let Some(ml) = maxlen {
                if cur_len >= ml {
                    return Err(PyError::named(
                        "IndexError",
                        "deque already at its maximum size".to_string(),
                    ));
                }
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
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let items = deque_items_snapshot(&inst)?;
            let len = items.len();
            let idx = deque_resolve_index(args[1].value.kind(), len, FN_NAME)?;
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
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let items_val = deque_items_val(&inst)?;
            let len = items_val.list_len().unwrap_or(0);
            let idx = deque_resolve_index(args[1].value.kind(), len, FN_NAME)?;
            items_val.list_pop_at(idx)?;
            Ok(Value::none())
        }

        /// `x in d` — membership test using `==` equality.
        fn __contains__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let target = args[1].value.clone();
            let items = deque_items_snapshot(&inst)?;
            for v in &items {
                if _interp.values_user_eq(v, &target)? {
                    return Ok(Value::bool_(true));
                }
            }
            Ok(Value::bool_(false))
        }

        /// `for x in d` — yield elements in left-to-right order.
        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let items = deque_items_snapshot(&inst)?;
            Ok(Value::generator(Box::new(NativeIterFrame { items, pos: 0 })))
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
                                _ => v.repr(),
                            }
                        } else {
                            v.repr()
                        }
                    }
                    _ => v.repr(),
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
            let attr_name = match args[1].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "attribute name must be string".to_string(),
                    ));
                }
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

    // `other` is a plain dict — use its values as deltas directly.
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
    } else if let Some(other_counts) = counts_of(other) {
        // `other` is a Counter (PyInstance with `_counts`) — treat its
        // stored counts as deltas, exactly like the plain-dict branch.
        // This fixes the case where `Counter.update(another_counter)` was
        // falling through to the iterable path (contributing +1 per key
        // rather than the actual stored count).
        for (k, v) in other_counts.iter() {
            let delta = match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "Counter delta values must be integers".to_string(),
                    ))
                }
            };
            let cur = counts.get(k).map(value_as_count).unwrap_or(0);
            counts.insert(k.clone(), Value::int(cur + sign * delta));
        }
    } else {
        // Iterable form — each element contributes ±1.
        for v in interp.collect_iterable(other.clone())? {
            let key = interp.value_to_pykey(&v)?;
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
        PyKey::BigInt(v) => Value::bigint(*v),
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

// ── deque helpers ────────────────────────────────────────────────────────────

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
