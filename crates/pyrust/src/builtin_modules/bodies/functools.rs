// `functools` module — body for the `functools` entry in
// `pyrust_builtin_modules!`.  Exposes `reduce`, `partial`, `lru_cache`,
// `wraps`, `cached_property`.  These are the four functools entries
// that didn't make it into the phase-2 stdlib drop (PR #327 → #329).
//
// ## Design choices
//
// - `partial` and the `lru_cache` wrappers are real Python classes
//   (defined via `pyrust_module!`'s `class { … }` block) with
//   `__init__` / `__call__` dunders.  Instances flow through the
//   interpreter's standard PyInstance dispatch — calling `p(arg)`
//   finds `__call__` on the class and invokes it with `self`
//   prepended.
//
// - `cached_property` is a `BuiltinObject` (state lives in
//   `pyrust-builtins/src/cached_property.rs`) because the descriptor
//   protocol — `__get__` lookup from `Interpreter::get_attr` — runs
//   before the user-class fallback.  Plugging into the same hook as
//   `property` keeps the hot attribute-lookup path tight; see
//   `env.rs::get_attr`.
//
// - `wraps(orig)` returns a `_wraps_partial` callable.  Calling it
//   with a wrapper function returns a `_wrapper_attrs` instance that
//   delegates `__call__` to the wrapper and exposes
//   `__name__` / `__doc__` from the original.  UserFunction's name
//   field is shared through an `Rc` and not in-place mutable, so we
//   wrap rather than mutate — that's the "minimal `wraps`" the
//   issue spec authorises (only `__name__` + `__doc__`, no full
//   attr-copy).
//
// Reference: <https://docs.python.org/3/library/functools.html>

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{iter_values, reject_keyword_args_expanded};
use crate::value::{PyInstance, PyKey, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: functools.reduce(function, iterable[, initializer]).
    /// Apply `function` of two arguments cumulatively to the items of
    /// `iterable` — left-to-right — so as to reduce the iterable to a
    /// single value.  With `initializer`, it's placed before the items.
    /// <https://docs.python.org/3/library/functools.html#functools.reduce>
    fn reduce(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 || args.len() > 3 {
            // CPython raises `TypeError` for wrong-arity calls.
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 2 or 3 arguments"),
            ));
        }
        let func = args[0].value.clone();
        let items = iter_values(args[1].value.clone())?;
        let mut iter = items.into_iter();
        let mut acc = if args.len() == 3 {
            // Initializer supplied → it's the seed; the iterable is folded
            // onto it from the left.  This is the only branch that's
            // happy with an empty iterable.
            args[2].value.clone()
        } else {
            // No initializer → first item is the seed.  Empty iterable
            // is a hard error (mirrors CPython's TypeError text).
            match iter.next() {
                Some(v) => v,
                None => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() of empty iterable with no initial value"),
                )),
            }
        };
        for item in iter {
            acc = _interp.call_function_expanded(
                func.clone(),
                &[
                    ExpandedCallArg { name: None, value: acc },
                    ExpandedCallArg { name: None, value: item },
                ],
            )?;
        }
        Ok(acc)
    }

    /// CPython: functools.partial(func, /, *args, **kwargs).
    /// Returns a callable that pre-binds the leading positional and
    /// keyword arguments; subsequent calls append additional positional
    /// args and merge additional kwargs (caller wins on conflict — that
    /// matches CPython's `partial.__call__` semantics).
    /// <https://docs.python.org/3/library/functools.html#functools.partial>
    class partial {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME} expected at least 1 argument, got 0"),
                ));
            }
            // First positional is `func`; remaining positional+keyword are
            // the pre-bound args/kwargs.  `func` itself is positional-only
            // in CPython (the `/` in the signature) so we don't accept
            // `func=` as a keyword.
            let func = user[0].value.clone();
            let mut bound_args: Vec<Value> = Vec::new();
            let mut bound_kwargs: IndexMap<PyKey, Value> = IndexMap::new();
            for a in &user[1..] {
                match &a.name {
                    Some(n) => {
                        bound_kwargs.insert(PyKey::Str(n.clone()), a.value.clone());
                    }
                    None => bound_args.push(a.value.clone()),
                }
            }
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("func".to_string(), func);
            attrs
                .attrs
                .insert("args".to_string(), Value::tuple(bound_args));
            attrs
                .attrs
                .insert("keywords".to_string(), Value::dict(bound_kwargs));
            Ok(Value::none())
        }

        /// `p(*args, **kwargs)` — pre-bind, then dispatch.  Pre-bound
        /// positional args come first, then call-site positional args.
        /// Kwargs merge with caller-wins semantics.
        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (func, bound_pos, bound_kw) = read_partial_state(&inst, FN_NAME)?;
            let user = &args[1..];
            let mut combined: Vec<ExpandedCallArg> =
                Vec::with_capacity(bound_pos.len() + user.len());
            for v in bound_pos {
                combined.push(ExpandedCallArg { name: None, value: v });
            }
            // Build the merged kwarg map keyed by name; caller's kwargs
            // overwrite pre-bound ones (matches CPython).
            let mut kw_map: IndexMap<String, Value> = IndexMap::new();
            for (k, v) in bound_kw {
                if let PyKey::Str(name) = k {
                    kw_map.insert(name, v);
                }
            }
            for a in user {
                match &a.name {
                    Some(n) => {
                        kw_map.insert(n.clone(), a.value.clone());
                    }
                    None => {
                        combined.push(ExpandedCallArg { name: None, value: a.value.clone() });
                    }
                }
            }
            for (name, value) in kw_map {
                combined.push(ExpandedCallArg { name: Some(name), value });
            }
            _interp.call_function_expanded(func, &combined)
        }
    }

    /// CPython: functools.lru_cache(maxsize=128, typed=False).
    /// May be used as `@lru_cache`, `@lru_cache()`, or
    /// `@lru_cache(maxsize=N, typed=T)`.  In the bare form the next
    /// positional arg is the function itself (callable) and we apply
    /// the defaults; in the parenthesised forms we return a decorator
    /// factory (`_lru_cache_factory`) that takes the function on its
    /// next call.
    /// <https://docs.python.org/3/library/functools.html#functools.lru_cache>
    fn lru_cache(args) -> Result<Value> {
        // Reject any unknown kwargs.
        for a in args.iter() {
            if let Some(name) = a.name.as_deref()
                && name != "maxsize"
                && name != "typed"
            {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() got an unexpected keyword argument '{name}'"),
                ));
            }
        }
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        let maxsize_kw = args.iter().find(|a| a.name.as_deref() == Some("maxsize"));
        let typed_kw = args.iter().find(|a| a.name.as_deref() == Some("typed"));

        // Bare-`@lru_cache` form: one positional callable, no kwargs.
        // CPython's heuristic — if the first arg is callable and no
        // configuration was passed, treat it as the function.
        if positional.len() == 1
            && maxsize_kw.is_none()
            && typed_kw.is_none()
            && is_callable(&positional[0].value)
        {
            return Ok(make_lru_wrapper(positional[0].value.clone(), Some(128), false));
        }
        if positional.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 1 positional argument"),
            ));
        }
        // Factory form.  `maxsize` may be an int, `None` (unbounded), or
        // absent (default 128).  `typed` may be a bool / int / absent.
        let maxsize_val = positional
            .first()
            .map(|a| a.value.clone())
            .or_else(|| maxsize_kw.map(|a| a.value.clone()));
        let maxsize: Option<i64> = match maxsize_val.as_ref().map(|v| v.kind()) {
            None => Some(128),
            Some(ValueKind::None) => None,
            Some(ValueKind::Int(n)) if n >= 0 => Some(n),
            Some(ValueKind::Int(_)) => return Err(PyError::named(
                "ValueError",
                format!("{FN_NAME}() maxsize must be non-negative"),
            )),
            Some(ValueKind::Bool(b)) => Some(b as i64),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() maxsize must be an integer or None"),
            )),
        };
        let typed = match typed_kw.map(|a| a.value.kind()) {
            None => false,
            Some(ValueKind::Bool(b)) => b,
            Some(ValueKind::Int(n)) => n != 0,
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() typed must be a bool"),
            )),
        };
        Ok(make_lru_factory(maxsize, typed))
    }

    /// Wrapper class produced by `lru_cache(func)` (or
    /// `lru_cache(maxsize=N)(func)`).  Callable; on each call builds a
    /// key from the args, returns the cached value on hit, computes &
    /// stores on miss.  LRU eviction when `maxsize` is bounded.
    class _lru_cache_wrapper {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            // Private constructor — only this module's helpers construct
            // instances of this class (seeding attrs directly via
            // `make_lru_wrapper`).  Reject any user-provided arguments so
            // calling `_lru_cache_wrapper(...)` from outside this module
            // fails loudly rather than producing a broken instance.
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments (got {})", args.len() - 1),
                ));
            }
            Ok(Value::none())
        }

        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            let (func, maxsize, typed) = {
                let borrow = inst.borrow();
                let func = borrow
                    .attrs
                    .get("_func")
                    .cloned()
                    .ok_or_else(|| internal(FN_NAME))?;
                let maxsize = match borrow.attrs.get("_maxsize").map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => Some(n),
                    Some(ValueKind::None) | None => None,
                    _ => return Err(internal(FN_NAME)),
                };
                let typed = matches!(
                    borrow.attrs.get("_typed").map(|v| v.kind()),
                    Some(ValueKind::Bool(true))
                );
                (func, maxsize, typed)
            };
            let key = build_key(user, typed);
            let key_pykey = PyKey::Str(key.clone());
            // Hit?  The `_cache` dict is keyed by `PyKey::Str(key)` and
            // `_order` is the LRU list (front = LRU, back = MRU).
            let hit = {
                let borrow = inst.borrow();
                borrow
                    .attrs
                    .get("_cache")
                    .and_then(|v| v.as_dict().map(|d| d.get(&key_pykey).cloned()))
                    .flatten()
            };
            if let Some(v) = hit {
                promote_key(&inst, &key);
                return Ok(v);
            }
            // Miss: compute, insert, evict if over capacity.
            let result = _interp.call_function_expanded(func, user)?;
            insert_cache(&inst, key, key_pykey, result.clone(), maxsize);
            Ok(result)
        }

        /// `wrapper.cache_clear()` — drop all cached entries.  Matches
        /// CPython's API.
        fn cache_clear(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let mut borrow = inst.borrow_mut();
            borrow
                .attrs
                .insert("_cache".to_string(), Value::dict(IndexMap::new()));
            borrow
                .attrs
                .insert("_order".to_string(), Value::list(Vec::new()));
            let _ = _interp;
            Ok(Value::none())
        }
    }

    /// Decorator factory produced by `lru_cache(maxsize=N)` /
    /// `lru_cache()`.  Calling it with the user's function returns the
    /// real `_lru_cache_wrapper`.
    class _lru_cache_factory {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            // Private constructor — `lru_cache(...)` constructs these
            // factories via `make_lru_factory`.  Reject user args so a
            // stray `_lru_cache_factory(...)` call fails loudly.
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments (got {})", args.len() - 1),
                ));
            }
            Ok(Value::none())
        }

        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let (maxsize, typed) = {
                let borrow = inst.borrow();
                let maxsize = match borrow.attrs.get("_maxsize").map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => Some(n),
                    Some(ValueKind::None) | None => None,
                    _ => return Err(internal(FN_NAME)),
                };
                let typed = matches!(
                    borrow.attrs.get("_typed").map(|v| v.kind()),
                    Some(ValueKind::Bool(true))
                );
                (maxsize, typed)
            };
            let _ = _interp;
            Ok(make_lru_wrapper(user[0].value.clone(), maxsize, typed))
        }
    }

    /// CPython: functools.wraps(wrapped).
    /// Returns a decorator that, when applied to `wrapper`, copies
    /// `wrapped.__name__` and `wrapped.__doc__` onto the wrapper.
    ///
    /// **Minimal implementation** — pyrust's `UserFunction.name` is
    /// shared through an `Rc` and not in-place mutable, so we return a
    /// wrapper-class instance that *exposes* the original's `__name__`
    /// and `__doc__` via attribute access and forwards `__call__` to
    /// the original wrapper.  This is the "good enough" shape called
    /// out in the issue spec (#329); the full attr-copy variant
    /// (`__module__`, `__qualname__`, `__dict__`) is out of scope here.
    fn wraps(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let orig = args[0].value.clone();
        let name = function_name(&orig).unwrap_or_else(|| "wrapper".to_string());
        let doc = function_doc(&orig);
        let _ = _interp;
        Ok(make_wraps_partial(Value::string(name), doc))
    }

    /// `wraps(orig)` returns one of these.  Calling it with a wrapper
    /// returns a `_wrapper_attrs` instance carrying the original's
    /// metadata and forwarding `__call__` to the wrapper.
    class _wraps_partial {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            // Private constructor — `wraps()` seeds attrs directly via
            // `make_wraps_partial`.  Reject user args so calling
            // `_wraps_partial(...)` from outside this module fails
            // loudly rather than producing a broken instance.
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments (got {})", args.len() - 1),
                ));
            }
            Ok(Value::none())
        }

        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME} expected exactly 1 argument"),
                ));
            }
            let (name, doc) = {
                let borrow = inst.borrow();
                (
                    borrow
                        .attrs
                        .get("__wraps_name")
                        .cloned()
                        .unwrap_or_else(|| Value::string("wrapper")),
                    borrow
                        .attrs
                        .get("__wraps_doc")
                        .cloned()
                        .unwrap_or_else(Value::none),
                )
            };
            let _ = _interp;
            Ok(make_wrapper_attrs(user[0].value.clone(), name, doc))
        }
    }

    /// The actual wrapper produced by `@wraps(orig) def wrapper(...)`.
    /// Exposes `__name__` and `__doc__` from `orig`; delegates the call
    /// to the inner wrapper function.
    class _wrapper_attrs {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            // Private constructor — `_wraps_partial.__call__` seeds the
            // attrs directly via `make_wrapper_attrs`.  Reject user args
            // so a stray `_wrapper_attrs(...)` call fails loudly.
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments (got {})", args.len() - 1),
                ));
            }
            Ok(Value::none())
        }

        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            let func = inst
                .borrow()
                .attrs
                .get("__wraps_func")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            _interp.call_function_expanded(func, user)
        }
    }

    /// CPython: functools.cached_property(func).
    /// Transforms a method into a property whose value is computed once
    /// per instance and stashed in the instance's `__dict__` so the
    /// next access bypasses the descriptor.  Implemented as a
    /// `BuiltinObject` (`pyrust_builtins::cached_property`) so the
    /// descriptor dispatch shares the same hot path as `property` in
    /// `env.rs::get_attr`.
    /// <https://docs.python.org/3/library/functools.html#functools.cached_property>
    fn cached_property(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let func = args[0].value.clone();
        // CPython grabs the attr name via `__set_name__(owner, name)`;
        // pyrust doesn't dispatch that dunder yet, so we fall back to
        // the wrapped function's own name — fine for the common
        // `@cached_property def x(self): …` shape (function name and
        // attribute name agree).
        //
        // Workaround: if you need the attribute name to differ from the
        // function's `__name__`, call `desc.__set_name__(cls, 'attr_name')`
        // after class creation (this is what the parity test does).
        let attr_name = function_name(&func).unwrap_or_else(|| "cached_property".to_string());
        let _ = _interp;
        Ok(pyrust_builtins::cached_property::cached_property(
            func, attr_name,
        ))
    }
}

// ── shared helpers ───────────────────────────────────────────────────────────

/// Method-shared `self` extractor — `args[0]` is the instance (the
/// `pyrust_module!` `class { … }` block always prepends it).
fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(&rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Internal-error shorthand.  Should never fire in practice; reaching
/// it means a class instance was constructed without going through its
/// `__init__` (or an attr was overwritten externally).
fn internal(fn_name: &str) -> PyError {
    PyError::Runtime(format!("internal: {fn_name}() instance state corrupted"))
}

/// Pull a class out of *this* module's `module()` build — the macro
/// emits a `module() -> Value` constructor that returns a fresh
/// `PyModule` with the class attrs already populated.  Each call
/// rebuilds the module, but the resulting `PyClass` carries the same
/// method-name → leaked-static-name mapping, so any instance we build
/// here will dispatch correctly under `calls.rs`'s PyInstance arm.
fn module_class(name: &str) -> Option<Rc<RefCell<crate::value::PyClass>>> {
    let module_val = module();
    let ValueKind::PyModule(m) = module_val.kind() else {
        return None;
    };
    let class_val = m.borrow().attrs.get(name).cloned()?;
    match class_val.kind() {
        ValueKind::PyClass(c) => Some(Rc::clone(&c)),
        _ => None,
    }
}

/// Construct a `PyInstance` of class `name` with the supplied attrs,
/// bypassing `__init__`.  Used by the LRU and wraps helpers below to
/// seed private state without going through a public constructor.
fn make_instance(name: &str, attrs: HashMap<String, Value>) -> Value {
    match module_class(name) {
        Some(class) => Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs }))),
        // "Shouldn't happen" really means "would indicate a macro/build
        // bug": every class name passed here is declared in this very
        // module via `class { … }`, which the `pyrust_module!` macro
        // wires into `module()`.  If a lookup miss ever escapes, it's
        // a build-time inconsistency — fail loud rather than handing
        // back a silently-broken `None`.
        None => unreachable!(
            "internal: functools module did not register class `{name}` \
             (declared via `class {{ … }}` in this module — macro build broken)",
        ),
    }
}

// ── partial helpers ──────────────────────────────────────────────────────────

fn read_partial_state(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(Value, Vec<Value>, IndexMap<PyKey, Value>)> {
    let borrow = inst.borrow();
    let func = borrow
        .attrs
        .get("func")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let args = match borrow.attrs.get("args").map(|v| v.kind()) {
        Some(ValueKind::Tuple(items)) => items.to_vec(),
        _ => return Err(internal(fn_name)),
    };
    let kwargs = match borrow.attrs.get("keywords").map(|v| v.kind()) {
        Some(ValueKind::Dict(d)) => d.clone(),
        _ => return Err(internal(fn_name)),
    };
    Ok((func, args, kwargs))
}

// ── lru_cache helpers ────────────────────────────────────────────────────────

/// Decide if `v` is "callable" in the sense that `lru_cache` should
/// treat its first positional arg as the function to wrap (the bare
/// `@lru_cache` form).  Anything else falls through to the
/// decorator-factory branch.
fn is_callable(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. }
            | ValueKind::PyClass(_)
    )
}

/// Build a string key from `args` for the LRU cache.  Each arg is
/// emitted as `[type:]repr`, joined by an ASCII unit-separator.  When
/// `typed=true`, the type tag is included so `1` and `1.0` map to
/// different keys (matches CPython's `typed=True` semantics).
fn build_key(args: &[ExpandedCallArg], typed: bool) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(args.len() * 2);
    for a in args.iter().filter(|a| a.name.is_none()) {
        parts.push(encode_value(&a.value, typed));
    }
    // Sentinel between positional and kwargs so `f(1, x=2)` and
    // `f(1, 2)` can never collide.
    parts.push("\x1e".to_string());
    // Keyword args sorted by name so `f(x=1, y=2)` and `f(y=2, x=1)`
    // produce the same key.
    let mut kw: Vec<(&str, &Value)> = args
        .iter()
        .filter_map(|a| a.name.as_deref().map(|n| (n, &a.value)))
        .collect();
    kw.sort_by_key(|(n, _)| *n);
    for (name, value) in kw {
        parts.push(name.to_string());
        parts.push(encode_value(value, typed));
    }
    parts.join("\x1f")
}

/// Encode a single value into a cache-key substring.  The type tag is
/// always present — CPython's `lru_cache(typed=False)` *also*
/// distinguishes `1` (int) from `1.0` (float) because its internal
/// `_make_key` wraps floats in a `_HashedSeq` with a different hash
/// than the bare int (so the keys never collide even though `1 ==
/// 1.0`).  The `typed=true` flag adds an *additional* sub-tag for
/// 1-arg fast-path types (mirroring CPython's `[type]` decoration).
fn encode_value(v: &Value, typed: bool) -> String {
    let (tag, body) = match v.kind() {
        ValueKind::None => ("n", "None".to_string()),
        ValueKind::Bool(b) => ("b", b.to_string()),
        ValueKind::Int(n) => ("i", n.to_string()),
        ValueKind::Float(f) => ("f", f.to_string()),
        ValueKind::Str(s) => ("s", format!("{:?}", s.to_string())),
        _ => ("o", v.repr()),
    };
    if typed {
        // typed=True adds explicit type-name segmentation so subclass
        // instances of int (e.g. bool) and the underlying int never
        // share a key.  Equivalent to CPython's "fasttypes" branch.
        format!("T{tag}:{body}")
    } else {
        format!("{tag}:{body}")
    }
}

/// Move `key` to the MRU end of `_order`.  No-op if the key isn't in
/// the order list (shouldn't happen — but we tolerate it rather than
/// panicking on the hot path).
fn promote_key(inst: &Rc<RefCell<PyInstance>>, key: &str) {
    let mut borrow = inst.borrow_mut();
    if let Some(order) = borrow.attrs.get_mut("_order").and_then(Value::as_list_mut) {
        if let Some(pos) = order
            .iter()
            .position(|v| matches!(v.kind(), ValueKind::Str(s) if s == key))
        {
            let item = order.remove(pos);
            order.push(item);
        }
    }
}

/// Insert `value` into the cache under `key`; if `maxsize` is bounded
/// and the cache is full, evict the oldest entry (front of `_order`).
fn insert_cache(
    inst: &Rc<RefCell<PyInstance>>,
    key: String,
    key_pykey: PyKey,
    value: Value,
    maxsize: Option<i64>,
) {
    let mut borrow = inst.borrow_mut();
    let order_full = if let Some(order) =
        borrow.attrs.get_mut("_order").and_then(Value::as_list_mut)
    {
        order.push(Value::string(&key));
        match maxsize {
            Some(0) => true,
            Some(n) => order.len() > n as usize,
            None => false,
        }
    } else {
        false
    };
    if let Some(cache) = borrow.attrs.get_mut("_cache").and_then(Value::as_dict_mut) {
        cache.insert(key_pykey, value);
    }
    // Evict head if over capacity.  `maxsize=0` is a degenerate case
    // — no entries are kept; we immediately evict what we just
    // inserted.
    if order_full {
        let evict_key: Option<String> = {
            let order = borrow.attrs.get_mut("_order").and_then(Value::as_list_mut);
            if let Some(order) = order
                && !order.is_empty()
            {
                let head = order.remove(0);
                match head.kind() {
                    ValueKind::Str(s) => Some(s.to_string()),
                    _ => None,
                }
            } else {
                None
            }
        };
        if let Some(k) = evict_key
            && let Some(cache) = borrow.attrs.get_mut("_cache").and_then(Value::as_dict_mut)
        {
            cache.shift_remove(&PyKey::Str(k));
        }
    }
}

/// Construct a `_lru_cache_wrapper` instance seeded with `func` /
/// `maxsize` / `typed`.
fn make_lru_wrapper(func: Value, maxsize: Option<i64>, typed: bool) -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert("_func".to_string(), func);
    attrs.insert(
        "_maxsize".to_string(),
        maxsize.map_or_else(Value::none, Value::int),
    );
    attrs.insert("_typed".to_string(), Value::bool_(typed));
    attrs.insert("_cache".to_string(), Value::dict(IndexMap::new()));
    attrs.insert("_order".to_string(), Value::list(Vec::new()));
    make_instance("_lru_cache_wrapper", attrs)
}

/// Construct a `_lru_cache_factory` instance — the decorator returned
/// by `lru_cache(maxsize=N)` / `lru_cache()`.
fn make_lru_factory(maxsize: Option<i64>, typed: bool) -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert(
        "_maxsize".to_string(),
        maxsize.map_or_else(Value::none, Value::int),
    );
    attrs.insert("_typed".to_string(), Value::bool_(typed));
    make_instance("_lru_cache_factory", attrs)
}

// ── wraps helpers ────────────────────────────────────────────────────────────

fn make_wraps_partial(name: Value, doc: Value) -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert("__wraps_name".to_string(), name);
    attrs.insert("__wraps_doc".to_string(), doc);
    make_instance("_wraps_partial", attrs)
}

fn make_wrapper_attrs(func: Value, name: Value, doc: Value) -> Value {
    let mut attrs: HashMap<String, Value> = HashMap::new();
    attrs.insert("__wraps_func".to_string(), func);
    // Stash the orig's name/doc under their dunder forms so
    // `wrapper.__name__` / `wrapper.__doc__` hit the instance-attrs
    // check in `env.rs::get_attr` (PyInstance arm, line ~37) and
    // return our cached value.
    attrs.insert("__name__".to_string(), name);
    attrs.insert("__doc__".to_string(), doc);
    make_instance("_wrapper_attrs", attrs)
}

/// Best-effort `__name__` extractor for `wraps`/`cached_property`.
/// UserFunctions carry the name in the struct; built-in functions
/// carry it in the kind tag.  Returns `None` for unknown kinds — the
/// caller substitutes a default.
fn function_name(v: &Value) -> Option<String> {
    match v.kind() {
        ValueKind::UserFunction(f) => Some(f.name.clone()),
        ValueKind::BuiltinFunction(s) => Some(s.to_string()),
        ValueKind::BoundMethod { function, .. } => Some(function.name.clone()),
        ValueKind::PyClass(c) => Some(c.borrow().name.clone()),
        _ => None,
    }
}

/// `function.__doc__` placeholder.  pyrust doesn't currently surface
/// docstrings on `UserFunction`, so we return `None` — the wrapper
/// then carries `__doc__ == None`.  That matches CPython's behaviour
/// when the wrapped function has no docstring.
fn function_doc(_v: &Value) -> Value {
    Value::none()
}
