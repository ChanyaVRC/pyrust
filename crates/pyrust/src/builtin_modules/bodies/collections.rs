// `collections` module — body for the `collections` entry in
// `pyrust_builtin_modules!`.
//
// Currently exposes `Counter` and `defaultdict`, both as proper
// `BuiltinTypeOps` types backed by `pyrust_builtins::counter` and
// `pyrust_builtins::defaultdict`.  The rest of the CPython
// `collections` API (`OrderedDict`, `deque`, `namedtuple`, `ChainMap`)
// is out of scope.
//
// `defaultdict`'s missing-key path bottoms out in
// `BuiltinTypeOps::missing_factory`, which the interpreter consults
// after a `KeyError` from `get_item`.  See `eval_index` in
// `interpreter/runtime/expr.rs` for the dispatch wiring.
//
// Reference: <https://docs.python.org/3/library/collections.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{iter_values, reject_keyword_args_expanded};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: collections.Counter([iterable_or_mapping]).
    ///
    /// Returns a real `Counter` instance with the full
    /// `most_common` / `elements` / `update` / `subtract` / `copy` /
    /// `keys` / `values` / `items` / `get` method surface and the
    /// missing-key-returns-0 dict-subclass semantics.  Arithmetic
    /// operators (`+`, `-`, `&`, `|`) are not yet supported.
    ///
    /// <https://docs.python.org/3/library/collections.html#collections.Counter>
    #[py_name = "Counter"]
    fn counter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes at most one argument",
            )));
        }
        let mut counts: indexmap::IndexMap<PyKey, i64> = indexmap::IndexMap::new();
        if let Some(arg) = args.first() {
            // Mapping form: keys → counts (integer values only).
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
                    counts.insert(k.clone(), count);
                }
            } else {
                // Iterable form: tally each element.
                for v in iter_values(arg.value.clone())? {
                    let key = v.to_key().ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("{FN_NAME}() elements must be hashable"),
                        )
                    })?;
                    *counts.entry(key).or_insert(0) += 1;
                }
            }
        }
        Ok(pyrust_builtins::counter::counter(counts))
    }

    /// CPython: collections.defaultdict(default_factory).
    ///
    /// Missing-key access calls `default_factory()` with no args,
    /// stores the result under the absent key, and returns it.  Passing
    /// `None` (or omitting `default_factory`) makes missing-key access
    /// raise `KeyError`, exactly like a plain `dict`.
    ///
    /// <https://docs.python.org/3/library/collections.html#collections.defaultdict>
    fn defaultdict(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        // CPython accepts (factory) or (factory, iterable_or_mapping).
        // We accept (factory) only for now — `dict(...)`-style seed
        // initialisation isn't yet supported on plain dicts either.
        if args.len() > 1 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes at most one argument",
            )));
        }
        let factory = args.first().map(|a| a.value.clone()).unwrap_or_else(Value::none);
        if !factory.is_none() {
            // Factory must be callable; cheap structural check to surface
            // a clear error at construction instead of at first missing-key.
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
        Ok(pyrust_builtins::defaultdict::defaultdict(
            factory,
            indexmap::IndexMap::new(),
        ))
    }
}
