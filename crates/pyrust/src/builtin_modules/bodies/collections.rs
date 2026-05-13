// `collections` module — body for the `collections` entry in
// `pyrust_builtin_modules!`.
//
// ## Scope of this initial landing
//
// The `Counter` function and `defaultdict` placeholder are both present;
// the rest of the CPython `collections` API (`OrderedDict`, `deque`,
// `namedtuple`, `ChainMap`) is out of scope.
//
// ### `Counter` — partial
//
// `Counter(iterable)` returns a *plain `dict`* whose values are the
// per-element counts.  This gives users the most common pattern
// (counting occurrences) without requiring the full Counter type:
//
//     counts = Counter([1, 2, 1, 3])   # → {1: 2, 2: 1, 3: 1}
//
// What's *not* available yet: the Counter-specific methods
// (`most_common`, `elements`, `update`, `subtract`) and the
// arithmetic operators (`+`, `-`, `&`, `|`).  Users who need
// `most_common(n)` can fall back to
// `sorted(counts.items(), key=lambda kv: -kv[1])[:n]`.
//
// Promoting `Counter` to a proper class with its own methods requires
// a new `BuiltinTypeOps` implementation in `pyrust-builtins` — tracked
// for a follow-up PR.
//
// ### `defaultdict` — deferred
//
// Faithful `defaultdict` needs the missing-key path
// (`d[absent_key]`) to call back into the interpreter to invoke the
// factory.  `BuiltinTypeOps::get_item` doesn't currently receive an
// interpreter reference, so the factory can't run from inside the
// dispatch.  Rather than ship a placebo (e.g. silently dropping the
// factory and behaving like `dict`), the constructor raises
// `NotImplementedError` so callers know they need a different
// approach.  Tracked for a follow-up that extends `BuiltinTypeOps`
// with an interpreter-aware hook.
//
// Reference: <https://docs.python.org/3/library/collections.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{iter_values, reject_keyword_args_expanded};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: collections.Counter(iterable) — return a `dict` whose
    /// values are the per-element counts.  Initial-landing simplification:
    /// the result is a plain `dict`, not a `Counter` instance with its
    /// own methods.  See the module-level docs.
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
        let mut counts: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
        if let Some(arg) = args.first() {
            // `Counter(mapping)` initialises from an existing
            // `key -> count` mapping (positive ints only, matching CPython).
            // Detect a dict before we attempt to iterate it as a sequence.
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
                // Otherwise iterate the source and tally each element.
                for v in iter_values(arg.value.clone())? {
                    let key = v.to_key().ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!("{FN_NAME}() elements must be hashable"),
                        )
                    })?;
                    let entry = counts.entry(key).or_insert_with(|| Value::int(0));
                    let next = match entry.kind() {
                        ValueKind::Int(n) => n.wrapping_add(1),
                        _ => unreachable!("Counter values are always Int by construction"),
                    };
                    *entry = Value::int(next);
                }
            }
        }
        Ok(Value::dict(counts))
    }

    /// CPython: collections.defaultdict(default_factory).
    ///
    /// Not yet supported — see the module-level docs for the rationale.
    /// The error explains the workaround so callers can adjust their
    /// code without spelunking through the internals.
    ///
    /// <https://docs.python.org/3/library/collections.html#collections.defaultdict>
    fn defaultdict(args) -> Result<Value> {
        let _ = args;
        Err(PyError::named(
            "NotImplementedError",
            format!(
                "{FN_NAME}() is not yet supported in pyrust — use a plain dict and \
                 check `key in d` (or `d.get(key, default)`) at each access site",
            ),
        ))
    }
}
