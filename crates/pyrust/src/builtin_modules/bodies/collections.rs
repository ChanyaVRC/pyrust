// `collections` module — body for the `collections` entry in
// `pyrust_builtin_modules!`.
//
// `Counter`, `defaultdict`, and `deque` are real Python classes (defined via
// `pyrust_module!`'s `class { … }` block).  Their dunder methods
// (`__init__`, `__getitem__`, `__iter__`, `__missing__`, …) plug into
// pyrust's standard class-method dispatch, so iteration / subscript /
// `isinstance` work without per-type plumbing in the interpreter.
//
// Native storage follows the class's actual data model:
//
// - **Counter** and **defaultdict** are real `dict` subclasses. Their mapping
//   is the standard builtin-subclass backing value (`__builtin_data__`), so
//   inherited dict operations, views, and native methods all see one map.
//   `defaultdict` additionally stores its callable-or-None
//   `self.default_factory`.
// - **deque**: `self._items` (opaque `VecDeque<Value>` storage) +
//   `self.maxlen` (an int ≥ 0 or None for unbounded).  `maxlen` is stored
//   directly under its public name so `d.maxlen` resolves via the normal
//   `attrs` lookup without any `__getattr__` plumbing.
//
// `defaultdict`'s missing-key path is the only place either class uses
// the new `__missing__` dunder: when `defaultdict.__getitem__` doesn't
// find the key, it calls `self.__missing__(key)`, which in turn runs
// the factory and stores the result.  CPython's exact mechanism.
//
// Reference: <https://docs.python.org/3/library/collections.html>

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    BUILTIN_DATA_ATTR, GuardVersion, Interpreter, IterSrcBuf, MapIter, NativeIterFrame,
    NativeIterGuard, class_is_subclass_of, coerce_subclass_backing, invoke_class_method,
    lookup_class_attr, value_from_bigint, value_type_name_str,
};
use crate::value::{
    InstanceAttrs, PyBigInt, PyClass, PyDict, PyInstance, PyKey, Value, ValueKind, key_repr,
};
use pyrust_derive::pyrust_module;

#[path = "collections/most_common.rs"]
mod most_common;
#[path = "collections/native_iterators.rs"]
mod native_iterators;

include!("collections/backing.rs");
include!("collections/canonical_identity.rs");
include!("collections/counter.rs");
include!("collections/deque.rs");
include!("collections/support.rs");

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

/// Public container classes tagged after module construction and the immutable
/// registry sentinel used for each class's PEP 585 adapter.
///
/// Keep the dispatch names as literals: `collections` is re-importable, so
/// allocating one permanently retained formatted string per class per module
/// generation would grow memory without bound.
const COLLECTION_CLASS_GETITEM_DISPATCH: [(&str, &str); 8] = [
    ("Counter", "Counter.__class_getitem__"),
    ("defaultdict", "defaultdict.__class_getitem__"),
    ("deque", "deque.__class_getitem__"),
    ("OrderedDict", "OrderedDict.__class_getitem__"),
    ("ChainMap", "ChainMap.__class_getitem__"),
    ("UserDict", "UserDict.__class_getitem__"),
    ("UserList", "UserList.__class_getitem__"),
    ("UserString", "UserString.__class_getitem__"),
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
    // CPython's collections module captures `itertools.chain` as `_chain` at
    // import time. Load that provider before tagging Counter so this
    // collections generation can retain the matching chain class even if
    // itertools is later removed from sys.modules and imported again.
    let itertools = interp.load_module("itertools")?;
    let chain = match itertools.kind() {
        ValueKind::PyModule(module) => module.borrow().attrs.get("chain").cloned(),
        _ => None,
    };
    let chain_class = match chain.as_ref().map(Value::kind) {
        Some(ValueKind::PyClass(class)) => Some(Rc::clone(class)),
        _ => None,
    };
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
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
    tag_public_classes(module, chain_class.as_ref());
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
fn tag_public_classes(
    module: &Rc<RefCell<crate::value::PyModule>>,
    chain_class: Option<&Rc<RefCell<PyClass>>>,
) {
    for (cls_name, class_getitem_dispatch) in COLLECTION_CLASS_GETITEM_DISPATCH {
        let cls = module.borrow().attrs.get(cls_name).cloned();
        if let Some(cls_val) = cls
            && let ValueKind::PyClass(cls_rc) = cls_val.kind()
        {
            cls_rc
                .borrow_mut()
                .attrs
                .insert("__module__".to_string(), Value::string("collections"));
            cls_rc.borrow_mut().attrs.insert(
                "__class_getitem__".to_string(),
                Value::builtin_function(class_getitem_dispatch),
            );
            if cls_name == "OrderedDict" {
                pyrust_builtins::ordered_mapping::register_class(cls_rc);
            }
            match cls_name {
                "Counter" => {
                    // Descriptor construction snapshots owner metadata through
                    // an immutable borrow. Complete it before opening the
                    // class-dict mutable borrow used for installation.
                    let fromkeys = pyrust_builtins::classmethod::native_class_method_descriptor(
                        Value::builtin_function("collections._counter_fromkeys"),
                        cls_rc,
                        "fromkeys",
                    );
                    cls_rc
                        .borrow_mut()
                        .attrs
                        .insert("fromkeys".to_string(), fromkeys);
                    register_canonical_collection_class(CanonicalCollectionKind::Counter, cls_rc);
                    if let Some(chain) = chain_class {
                        register_counter_chain_class(cls_rc, chain);
                    }
                }
                "deque" => {
                    register_canonical_collection_class(CanonicalCollectionKind::Deque, cls_rc)
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn test_class(name: &str, base: Option<Rc<RefCell<PyClass>>>) -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            qualname: name.to_string(),
            base,
            ..PyClass::default()
        }))
    }

    #[test]
    fn class_getitem_dispatch_names_are_static_and_complete() {
        assert_eq!(
            COLLECTION_CLASS_GETITEM_DISPATCH,
            [
                ("Counter", "Counter.__class_getitem__"),
                ("defaultdict", "defaultdict.__class_getitem__"),
                ("deque", "deque.__class_getitem__"),
                ("OrderedDict", "OrderedDict.__class_getitem__"),
                ("ChainMap", "ChainMap.__class_getitem__"),
                ("UserDict", "UserDict.__class_getitem__"),
                ("UserList", "UserList.__class_getitem__"),
                ("UserString", "UserString.__class_getitem__"),
            ]
        );

        let owner = include_str!("collections.rs");
        let forbidden = concat!("Box", "::leak");
        assert!(
            !owner.contains(forbidden),
            "re-importable collections generations must not leak dispatch names"
        );
    }

    #[test]
    fn retained_counter_resolves_base_from_its_own_generation() {
        let old_counter = test_class("old Counter", None);
        register_canonical_collection_class(CanonicalCollectionKind::Counter, &old_counter);

        let new_counter = test_class("new Counter", None);
        register_canonical_collection_class(CanonicalCollectionKind::Counter, &new_counter);

        let old_counter_subclass = test_class("old Counter child", Some(Rc::clone(&old_counter)));
        assert!(
            Rc::ptr_eq(
                &canonical_collection_base_for_receiver(
                    &old_counter,
                    CanonicalCollectionKind::Counter,
                )
                .unwrap(),
                &old_counter,
            )
        );
        assert!(Rc::ptr_eq(
            &canonical_collection_base_for_receiver(
                &old_counter_subclass,
                CanonicalCollectionKind::Counter,
            )
            .unwrap(),
            &old_counter,
        ));
        assert!(
            Rc::ptr_eq(
                &canonical_collection_base_for_receiver(
                    &new_counter,
                    CanonicalCollectionKind::Counter,
                )
                .unwrap(),
                &new_counter,
            )
        );
    }
}

pyrust_module! {
    constants {
        // Expose the `collections.abc` submodule as `collections.abc` so that
        // `import collections; collections.abc` resolves correctly.  The
        // `load_module` parent-package identity fix-up in `env.rs` replaces
        // this with the cached submodule value on first import, ensuring
        // `collections.abc is collections.abc` identity holds.
        "abc" => super::collections_abc::module()
    }

    /// Private adapter used by Counter.elements' lazy map/chain composition.
    ///
    /// Its input is one `(element, count)` pair from the live items iterator;
    /// the output is a finite repeat iterator. Count coercion therefore occurs
    /// only when that key is reached, exactly like CPython's
    /// `chain.from_iterable(starmap(repeat, self.items()))`.
    fn _counter_repeat_entry(args) -> Result<Value> {
        if args.len() != 1 || args[0].name.is_some() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one positional argument"),
            ));
        }
        let (element, count) = match args[0].value.kind() {
            ValueKind::Tuple(items) if items.len() == 2 => {
                (items[0].clone(), items[1].clone())
            }
            _ => {
                return Err(PyError::Runtime(
                    "internal: Counter.items() yielded a non-pair".to_string(),
                ));
            }
        };
        let count = _interp.value_to_isize(
            &count,
            "Python int too large to convert to C ssize_t",
        )?;
        Ok(native_iterators::repeat(element, count.max(0) as usize))
    }

    /// Native target wrapped as `Counter.fromkeys`' classmethod descriptor by
    /// `tag_public_classes`.  Counter intentionally disables dict.fromkeys:
    /// equal input elements make a single fixed value ambiguous as a count.
    fn _counter_fromkeys(args) -> Result<Value> {
        // The classmethod descriptor prepends `cls`.
        if args.len() < 2 {
            return Err(PyError::named(
                "TypeError",
                "Counter.fromkeys() missing 1 required positional argument: 'iterable'",
            ));
        }
        if args.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "Counter.fromkeys() takes from 2 to 3 positional arguments but {} were given",
                    args.len()
                ),
            ));
        }
        Err(PyError::named(
            "NotImplementedError",
            "Counter.fromkeys() is undefined.  Use Counter(iterable) instead.",
        ))
    }

    class Counter {
        /// CPython: Counter([iterable_or_mapping], **kwds) — tally elements.
        /// A positional iterable/mapping is tallied first, then any keyword
        /// arguments are *added* on top as string-keyed counts (#2013).
        /// <https://docs.python.org/3/library/collections.html#collections.Counter>
        fn __init__(args) -> Result<Value> {
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
            // Counter.__init__ delegates to update on the existing dict
            // backing.  Explicitly calling __init__ a second time therefore
            // preserves old counts and commits a failing iterable's completed
            // prefix; constructing a detached temporary map here lost both
            // behaviours and made infinite sources impossible to observe
            // incrementally.
            let backing = counter_backing(args, FN_NAME)?;
            if let Some(arg) = positional.first() {
                counter_tally_into_backing::<false>(_interp, &backing, &arg.value)?;
            }
            // Keyword arguments become string-keyed counts, added on top
            // (CPython `Counter('ab', a=10)` → a:11, b:1).
            counter_apply_kwargs_to_backing::<false>(_interp, &backing, &kwargs)?;
            Ok(Value::none())
        }

        /// Missing-key returns `0` — the dict-subclass quirk that makes
        /// Counter Counter.  This is the *only* defaulting branch; for
        /// proper present-key lookup we fall through to the stored map.
        fn __getitem__(args) -> Result<Value> {
            let backing = counter_backing(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            // Probe the live backing directly.  `dict_lookup` keeps primitive
            // keys O(1) and drops its map borrow before object-key `__eq__`.
            Ok(_interp
                .dict_lookup(&backing, &key)?
                .map(|(_, value)| value)
                .unwrap_or_else(|| Value::int(0)))
        }

        /// `c[k] = v` — store any value under the key (CPython does not
        /// enforce integer-only counts in `__setitem__`; it is merely
        /// conventional to store integers).
        fn __setitem__(args) -> Result<Value> {
            expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::Runtime(format!(
                    "{FN_NAME}() takes exactly 2 arguments",
                )));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let value = args[2].value.clone();
            let backing = counter_backing(args, FN_NAME)?;
            _interp.dict_insert_value(&backing, key, value)?;
            Ok(Value::none())
        }

        /// Counter deletion is intentionally idempotent: unlike dict,
        /// deleting a missing count does not raise `KeyError`.
        fn __delitem__(args) -> Result<Value> {
            expect_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let backing = counter_backing(args, FN_NAME)?;
            if let Some((index, _)) = _interp.dict_lookup(&backing, &key)? {
                backing
                    .dict_with_mut(|counts| {
                        counts.shift_remove_index(index);
                    })
                    .ok_or_else(|| {
                        PyError::Runtime("internal: Counter backing is not a dict".to_string())
                    })?;
            }
            Ok(Value::none())
        }

        /// `key in c` — fall through to the stored map's contains.
        fn __contains__(args) -> Result<Value> {
            let backing = counter_backing(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            Ok(Value::bool_(
                _interp.dict_lookup(&backing, &key)?.is_some(),
            ))
        }

        /// `len(c)` — number of stored entries.
        fn __len__(args) -> Result<Value> {
            let backing = counter_backing(args, FN_NAME)?;
            let len = backing.dict_with(|counts| counts.len()).ok_or_else(|| {
                PyError::Runtime("internal: Counter backing is not a dict".to_string())
            })?;
            Ok(Value::int(len as i64))
        }

        /// `for k in c` — a lazy live dict-key iterator in insertion order.
        fn __iter__(args) -> Result<Value> {
            let backing = counter_backing(args, FN_NAME)?;
            Ok(make_guarded_dict_subclass_iter(backing))
        }

        /// `repr(c)` — reuse the exact fallible ordering policy from
        /// `most_common()`.  CPython falls back to insertion order for a
        /// `TypeError` only; unrelated exceptions from user comparison code
        /// must propagate.
        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let backing = counter_backing(args, FN_NAME)?;
            let pairs: Vec<(PyKey, Value)> = backing
                .dict_with(|counts| {
                    counts
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                })
                .ok_or_else(|| {
                    PyError::Runtime("internal: Counter backing is not a dict".to_string())
                })?;
            if pairs.is_empty() {
                return Ok(Value::string(format!(
                    "{}()",
                    inst.borrow().class.borrow().name
                )));
            }
            let ordered = match most_common::select(_interp, pairs.clone(), None) {
                Ok(ordered) => ordered,
                Err(error) if error.class_name_is("TypeError") => pairs,
                Err(error) => return Err(error),
            };
            let inner: Vec<String> = ordered
                .iter()
                .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr_raw()))
                .collect();
            Ok(Value::string(format!(
                "{}({{{}}})",
                inst.borrow().class.borrow().name,
                inner.join(", ")
            )))
        }

        /// `c.most_common(n=None)` — list of (element, count) pairs in
        /// descending-count order.  `n=None` yields all entries.
        fn most_common(args) -> Result<Value> {
            let user = &args[1..];
            if user.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    "most_common() takes at most 1 argument".to_string(),
                ));
            }
            let backing = counter_backing(args, FN_NAME)?;
            let pairs: Vec<(PyKey, Value)> = backing
                .dict_with(|counts| {
                    counts
                        .iter()
                        .map(|(key, count)| (key.clone(), count.clone()))
                        .collect()
                })
                .ok_or_else(|| {
                    PyError::Runtime("internal: Counter backing is not a dict".to_string())
                })?;
            let selected = most_common::select(
                _interp,
                pairs,
                user.first().map(|argument| &argument.value),
            )?;
            Ok(Value::list(
                selected
                    .into_iter()
                    .map(|(key, count)| Value::tuple(vec![key_to_value(key), count]))
                    .collect(),
            ))
        }

        /// `c.elements()` — lazily yields each element `count` times, for
        /// elements whose count is `> 0`.
        fn elements(args) -> Result<Value> {
            let user = &args[1..];
            if !user.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "elements() takes no arguments".to_string(),
                ));
            }
            let counter_class = Rc::clone(&expect_self(args, FN_NAME)?.borrow().class);
            let chain_class = counter_elements_chain_class(&counter_class);
            let backing = counter_backing(args, FN_NAME)?;
            // A live dict-items iterator preserves the two observable pieces
            // of CPython behaviour: value-only mutations affect counts for
            // keys not reached yet, while size changes raise when the iterator
            // is next driven.
            let items_view = Interpreter::dict_view_for_backing(&backing, "items", false)?;
            let items_iter = crate::interpreter::make_iterator(_interp, &items_view)?;
            let mut sources = IterSrcBuf::new();
            sources.push(items_iter);
            let repeaters = Value::generator(Box::new(MapIter {
                func: Value::builtin_function("collections._counter_repeat_entry"),
                sources,
                done: false,
            }));
            Ok(native_iterators::elements(chain_class, repeaters))
        }

        /// `c.update([iterable_or_mapping], **kwds)` — add to counts (mapping
        /// form uses values as deltas; iterable form adds 1 per element; any
        /// keyword arguments are added on top as string-keyed counts, #2013).
        fn update(args) -> Result<Value> {
            apply_delta::<false>(_interp, args, FN_NAME)
        }

        /// `c.subtract([iterable_or_mapping], **kwds)` — subtract counts; the
        /// result can go below zero (`elements()` then skips them).  Keyword
        /// arguments are subtracted as string-keyed counts (#2013).
        fn subtract(args) -> Result<Value> {
            apply_delta::<true>(_interp, args, FN_NAME)
        }

        /// `c.total()` — sum of the counts (Python 3.10+).
        /// <https://docs.python.org/3/library/collections.html#collections.Counter.total>
        fn total(args) -> Result<Value> {
            require_no_args(args, "total")?;
            let backing = counter_backing(args, FN_NAME)?;
            let values = Interpreter::dict_view_for_backing(&backing, "values", false)?;
            _interp.call_function_expanded(
                Value::builtin_function("sum"),
                &[ExpandedCallArg {
                    name: None,
                    value: values,
                }],
            )
        }

        /// `c.copy()` — return a new Counter with the same counts.
        fn copy(args) -> Result<Value> {
            let counts = snapshot_counts(args, FN_NAME)?;
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
            attrs.insert(BUILTIN_DATA_ATTR, Value::dict(counts));
            Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class,
                attrs,
            }))))
        }

        /// `c.get(key, default=None)` — present-key lookup without the
        /// missing-key→0 default that `c[key]` applies.  Mirrors
        /// `dict.get`.
        fn get(args) -> Result<Value> {
            let backing = counter_backing(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    "get() takes 1 or 2 arguments".to_string(),
                ));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            match _interp.dict_lookup(&backing, &key)? {
                Some((_, value)) => Ok(value),
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
            let backing = counter_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "keys", false)
        }

        fn values(args) -> Result<Value> {
            require_no_args(args, "values")?;
            let backing = counter_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "values", false)
        }

        fn items(args) -> Result<Value> {
            require_no_args(args, "items")?;
            let backing = counter_backing(args, FN_NAME)?;
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

        /// Unary multiset normalization.  Both operations deliberately return
        /// a base Counter, matching CPython even when `self` is a subclass.
        fn __pos__(args) -> Result<Value> {
            counter_unary(_interp, args, CounterUnaryOp::Positive)
        }

        fn __neg__(args) -> Result<Value> {
            counter_unary(_interp, args, CounterUnaryOp::Negative)
        }

        /// Counter comparisons treat absent entries as zero and evaluate
        /// count rich-comparison methods in Counter insertion order.
        fn __eq__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Eq)
        }

        fn __ne__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Ne)
        }

        fn __le__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Le)
        }

        fn __lt__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Lt)
        }

        fn __ge__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Ge)
        }

        fn __gt__(args) -> Result<Value> {
            counter_compare(_interp, args, CounterCompareOp::Gt)
        }

        /// `c += d` — mutate the live dict backing in place and return `self`,
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
        /// Stores `self.default_factory` and initializes the dict backing.
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
            // CPython updates default_factory before it starts consuming the
            // dict initializer.  If that source fails, the new factory and
            // every pair already inserted remain visible on the existing
            // mapping.
            inst.borrow_mut()
                .attrs
                .insert("default_factory", factory);
            let backing = defaultdict_backing(args, FN_NAME)?;
            dict_init_into_backing(
                _interp,
                &backing,
                dict_positionals.first().map(|a| &a.value),
                &kwargs,
            )?;
            Ok(Value::none())
        }

        /// Subscripted access — on miss, calls `__missing__` (which runs
        /// the factory) rather than raising KeyError directly.  Matches
        /// CPython's dict-subclass semantics where `defaultdict[k]` =
        /// `dict.__getitem__(self, k)` falls back to `self.__missing__(k)`.
        fn __getitem__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let backing = defaultdict_backing(args, FN_NAME)?;
            if let Some((_, value)) = _interp.dict_lookup(&backing, &key)? {
                return Ok(value);
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
            let backing = defaultdict_backing(args, FN_NAME)?;
            _interp.dict_insert_value(&backing, pk, new_val.clone())?;
            Ok(new_val)
        }

        /// `d[k] = v` — straight write-through to the inner dict.
        fn __setitem__(args) -> Result<Value> {
            expect_self(args, FN_NAME)?;
            if args.len() != 3 {
                return Err(PyError::Runtime(format!(
                    "{FN_NAME}() takes exactly 2 arguments",
                )));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            let backing = defaultdict_backing(args, FN_NAME)?;
            _interp.dict_insert_value(&backing, key, args[2].value.clone())?;
            Ok(Value::none())
        }

        fn __contains__(args) -> Result<Value> {
            let backing = defaultdict_backing(args, FN_NAME)?;
            let key = require_key(_interp, args, 1, FN_NAME)?;
            Ok(Value::bool_(
                _interp.dict_lookup(&backing, &key)?.is_some(),
            ))
        }

        fn __len__(args) -> Result<Value> {
            let backing = defaultdict_backing(args, FN_NAME)?;
            let len = backing.dict_with(|items| items.len()).ok_or_else(|| {
                PyError::Runtime("internal: defaultdict backing is not a dict".to_string())
            })?;
            Ok(Value::int(len as i64))
        }

        fn __iter__(args) -> Result<Value> {
            let backing = defaultdict_backing(args, FN_NAME)?;
            Ok(make_guarded_dict_subclass_iter(backing))
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
            let backing = defaultdict_backing(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    "get() takes 1 or 2 arguments".to_string(),
                ));
            }
            let key = require_key(_interp, args, 1, FN_NAME)?;
            Ok(match _interp.dict_lookup(&backing, &key)? {
                Some((_, value)) => value,
                None => user.get(1).cloned().map(|a| a.value).unwrap_or_else(Value::none),
            })
        }

        // keys/values/items return LIVE dict views sharing the backing Rc.
        // #2436 made these live but tagged them `ordered=true`; that wording
        // ("OrderedDict mutated during iteration") never surfaced because the
        // stale-Rc replacement once detached the view before any guard
        // could fire.  defaultdict is a PLAIN `dict` subclass in CPython, so the
        // guard wording is the plain "dictionary changed size during iteration"
        // — `ordered=false` (issue #2447).
        fn keys(args) -> Result<Value> {
            require_no_args(args, "keys")?;
            let backing = defaultdict_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "keys", false)
        }

        fn values(args) -> Result<Value> {
            require_no_args(args, "values")?;
            let backing = defaultdict_backing(args, FN_NAME)?;
            crate::Interpreter::dict_view_for_backing(&backing, "values", false)
        }

        fn items(args) -> Result<Value> {
            require_no_args(args, "items")?;
            let backing = defaultdict_backing(args, FN_NAME)?;
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
            attrs.insert(BUILTIN_DATA_ATTR, Value::dict(items));
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
        ///   `self._items`  — opaque, Rc-shared `VecDeque<Value>` storage.
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
            let maxlen: Option<i64> = if let Some(raw) = raw_maxlen.as_ref()
                && !raw.is_none()
            {
                // CPython's deque constructor deliberately requires an actual
                // int (including int subclasses), unlike rotate/index which
                // accept arbitrary `__index__` providers.
                let normalized = match raw.kind() {
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => raw.clone(),
                    ValueKind::PyInstance(_) => coerce_subclass_backing(raw, &[])
                        .filter(|backing| {
                            matches!(
                                backing.kind(),
                                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                            )
                        })
                        .ok_or_else(|| {
                            PyError::named("TypeError", "an integer is required".to_string())
                        })?,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "an integer is required".to_string(),
                        ));
                    }
                };
                let value = _interp.value_to_isize(
                    &normalized,
                    "Python int too large to convert to C ssize_t",
                )?;
                if value < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        "maxlen must be non-negative".to_string(),
                    ));
                }
                Some(value)
            } else {
                None
            };
            // Store `maxlen` under the public name so `d.maxlen` resolves directly.
            let maxlen_val = match maxlen {
                Some(n) => Value::int(n),
                None => Value::none(),
            };
            {
                let mut attrs = inst.borrow_mut();
                if let Some(previous) = attrs.attrs.get("_items") {
                    // Reinitialising a deque is a structural mutation for any
                    // iterator that still observes the previous storage.
                    pyrust_builtins::deque_storage::bump_mutation_state(previous);
                }
                // Install the cleared deque and the new bound *before*
                // obtaining/advancing the source iterator.  Thus an iter()
                // failure leaves the deque empty with its new maxlen, and
                // d.__init__(d) sees the already-cleared receiver, matching
                // CPython.
                attrs.attrs.insert(
                    "_items",
                    pyrust_builtins::deque_storage::deque_storage(Vec::new()),
                );
                attrs.attrs.insert("maxlen", maxlen_val);
            }
            if let Some(iterable) = pos_iterable {
                deque_extend_iterable(_interp, &inst, &iterable, false)?;
            }
            Ok(Value::none())
        }

        /// `d.append(x)` — add to the right end.  When maxlen is set and
        /// the deque is full, the leftmost element is dropped.
        fn append(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let x = arg.clone();
            let maxlen = deque_maxlen(&inst);
            if let Some(0) = maxlen {
                // CPython treats the attempted append as a mutation even
                // though a zero-capacity deque remains empty.
                deque_bump_state(&inst);
                return Ok(Value::none()); // maxlen=0: discard all appends
            }
            let items = deque_items_data(&inst)?;
            let mut items = items.borrow_mut();
            if let Some(ml) = maxlen
                && items.len() >= ml
            {
                items.pop_front();
            }
            items.push_back(x);
            drop(items);
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
                // See `append`: an existing iterator must be invalidated.
                deque_bump_state(&inst);
                return Ok(Value::none()); // maxlen=0: discard all appends
            }
            let items = deque_items_data(&inst)?;
            let mut items = items.borrow_mut();
            if let Some(ml) = maxlen
                && items.len() >= ml
            {
                items.pop_back();
            }
            items.push_front(x);
            drop(items);
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `d.pop()` — remove and return from the right.  Raises
        /// `IndexError` if the deque is empty.
        fn pop(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items = deque_items_data(&inst)?;
            let popped = items.borrow_mut().pop_back().ok_or_else(|| {
                PyError::named("IndexError", "pop from an empty deque".to_string())
            })?;
            deque_bump_state(&inst);
            Ok(popped)
        }

        /// `d.popleft()` — remove and return from the left.  Raises
        /// `IndexError` if the deque is empty.
        fn popleft(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items = deque_items_data(&inst)?;
            let popped = items.borrow_mut().pop_front().ok_or_else(|| {
                PyError::named("IndexError", "pop from an empty deque".to_string())
            })?;
            deque_bump_state(&inst);
            Ok(popped)
        }

        /// `d.extend(iterable)` — extend right from an iterable, applying
        /// maxlen trimming along the way (same as repeated `append`).
        fn extend(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            deque_extend_iterable(_interp, &inst, arg, false)
        }

        /// `d.extendleft(iterable)` — extend left from an iterable,
        /// prepending each element in turn (which reverses the iterable's
        /// order — matching CPython).  Maxlen trimming from the right.
        fn extendleft(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            deque_extend_iterable(_interp, &inst, arg, true)
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
            let n = if let Some(arg) = user.first() {
                _interp.value_to_isize(
                    &arg.value,
                    "Python int too large to convert to C ssize_t",
                )?
            } else {
                1
            };
            let items = deque_items_data(&inst)?;
            let len = items.borrow().len();
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
                items.borrow_mut().rotate_right(steps);
            }
            Ok(Value::none())
        }

        /// `d.clear()` — remove all elements.  maxlen is preserved.
        fn clear(args) -> Result<Value> {
            let inst = expect_self_no_args(args, FN_NAME)?;
            let items = deque_items_data(&inst)?;
            let changed = !items.borrow().is_empty();
            if changed {
                items.borrow_mut().clear();
                deque_bump_state(&inst);
            }
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
            attrs.insert(
                "_items",
                pyrust_builtins::deque_storage::deque_storage(items),
            );
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
            let (items, mutation_state, version) =
                deque_items_snapshot_guarded(&inst)?;
            let mut n: i64 = 0;
            for v in &items {
                let equal = _interp.values_user_eq(v, &target)?;
                deque_require_unmutated(&mutation_state, version, "RuntimeError")?;
                if equal {
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
            let (items, mutation_state, version) =
                deque_items_snapshot_guarded(&inst)?;
            let mut found: Option<usize> = None;
            for (i, v) in items.iter().enumerate() {
                let equal = _interp.values_user_eq(v, &target)?;
                deque_require_unmutated(&mutation_state, version, "IndexError")?;
                if equal {
                    found = Some(i);
                    break;
                }
            }
            match found {
                Some(i) => {
                    deque_items_data(&inst)?.borrow_mut().remove(i);
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
            let items = deque_items_data(&inst)?;
            items.borrow_mut().make_contiguous().reverse();
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
            let start_arg = if let Some(arg) = user.get(1) {
                Some(deque_resolve_search_bound(_interp, &arg.value)?)
            } else {
                None
            };
            let stop_arg = if let Some(arg) = user.get(2) {
                Some(deque_resolve_search_bound(_interp, &arg.value)?)
            } else {
                None
            };
            let (items, mutation_state, version) =
                deque_items_snapshot_guarded(&inst)?;
            let len = items.len();
            let start = start_arg
                .as_ref()
                .map_or(0, |value| deque_normalize_search_bound(value, len));
            let stop = stop_arg
                .as_ref()
                .map_or(len, |value| deque_normalize_search_bound(value, len));
            for (i, item) in items.iter().enumerate().take(stop).skip(start) {
                let equal = _interp.values_user_eq(item, &target)?;
                deque_require_unmutated(&mutation_state, version, "RuntimeError")?;
                if equal {
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
            let i = _interp.value_to_isize(
                &args[1].value,
                "Python int too large to convert to C ssize_t",
            )?;
            let maxlen = deque_maxlen(&inst);
            let items = deque_items_data(&inst)?;
            let cur_len = items.borrow().len();
            if let Some(ml) = maxlen
                && cur_len >= ml {
                    return Err(PyError::named(
                        "IndexError",
                        "deque already at its maximum size".to_string(),
                    ));
                }
            let x = args[2].value.clone();
            // Clamp index like CPython: negative clamps to 0, beyond end
            // clamps to len.
            let idx = if i < 0 {
                (cur_len as i64 + i).max(0) as usize
            } else {
                (i as usize).min(cur_len)
            };
            items.borrow_mut().insert(idx, x);
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `len(d)` — number of elements.
        fn __len__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            Ok(Value::int(deque_items_data(&inst)?.borrow().len() as i64))
        }

        /// `d[i]` — element at index `i`.  Negative indices count from the
        /// right.  Raises `IndexError` if out of range.
        fn __getitem__(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let index = deque_index_i64(_interp, arg)?;
            let items = deque_items_data(&inst)?;
            let len = items.borrow().len();
            let idx = deque_normalize_index(index, len)?;
            let item = items.borrow().get(idx).cloned().ok_or_else(|| {
                PyError::Runtime("internal: resolved deque index disappeared".to_string())
            })?;
            Ok(item)
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
            let index = deque_index_i64(_interp, &args[1].value)?;
            let items = deque_items_data(&inst)?;
            let len = items.borrow().len();
            let idx = deque_normalize_index(index, len)?;
            let x = args[2].value.clone();
            items.borrow_mut()[idx] = x;
            Ok(Value::none())
        }

        /// `del d[i]` — delete element at index `i`.
        fn __delitem__(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let index = deque_index_i64(_interp, arg)?;
            let items = deque_items_data(&inst)?;
            let len = items.borrow().len();
            let idx = deque_normalize_index(index, len)?;
            items.borrow_mut().remove(idx);
            deque_bump_state(&inst);
            Ok(Value::none())
        }

        /// `x in d` — membership test using `==` equality.
        fn __contains__(args) -> Result<Value> {
            let (inst, arg) = expect_self_one_arg(args, FN_NAME)?;
            let target = arg.clone();
            let (items, mutation_state, version) =
                deque_items_snapshot_guarded(&inst)?;
            for v in &items {
                let equal = _interp.values_user_eq(v, &target)?;
                deque_require_unmutated(&mutation_state, version, "RuntimeError")?;
                if equal {
                    return Ok(Value::bool_(true));
                }
            }
            Ok(Value::bool_(false))
        }

        /// `for x in d` — yield elements in left-to-right order.
        ///
        /// The iterator indexes the live `VecDeque` without first cloning all
        /// elements and guards it with the deque's mutation counter.  Any
        /// structural change makes the next step raise
        /// `RuntimeError: deque mutated during iteration`, matching CPython.
        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let storage = deque_storage_value(&inst)?;
            let items = pyrust_builtins::deque_storage::data(&storage).ok_or_else(|| {
                PyError::Runtime("internal: deque storage lost its buffer".to_string())
            })?;
            let counter =
                pyrust_builtins::deque_storage::mutation_state(&storage).ok_or_else(|| {
                    PyError::Runtime("internal: deque storage lost its mutation state".to_string())
                })?;
            let version = counter.get();
            let mut frame = NativeIterFrame::deque(items, "generator");
            frame.guard = Some(Box::new(NativeIterGuard {
                container: storage,
                version,
                kind: GuardVersion::DequeState { counter },
                msg: "deque mutated during iteration",
                exhaust_first: false,
                provider_sequence: 0,
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
            let other_inst = match other.kind() {
                ValueKind::PyInstance(other_inst)
                    if is_canonical_collection_class_or_subclass(
                        &other_inst.borrow().class,
                        CanonicalCollectionKind::Deque,
                    ) =>
                {
                    Rc::clone(other_inst)
                }
                _ => return Ok(Value::not_implemented()),
            };
            let (self_items, self_state, self_version) =
                deque_items_snapshot_guarded(&inst)?;
            let (other_items, other_state, other_version) =
                deque_items_snapshot_guarded(&other_inst)?;
            if self_items.len() != other_items.len() {
                return Ok(Value::bool_(false));
            }
            for (a, b) in self_items.iter().zip(other_items.iter()) {
                let equal = _interp.values_user_eq(a, b)?;
                if !equal {
                    // CPython returns immediately on a mismatch; a mutation
                    // performed by that final false comparison is not checked.
                    return Ok(Value::bool_(false));
                }
                deque_require_unmutated(&self_state, self_version, "RuntimeError")?;
                deque_require_unmutated(&other_state, other_version, "RuntimeError")?;
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
