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
// - `wraps(orig)` returns a `_wraps_partial` callable.  Calling it with
//   the wrapper function mutates that wrapper in place — copying the
//   WRAPPER_ASSIGNMENTS attrs (`__module__`/`__name__`/`__qualname__`/
//   `__annotations__`/`__doc__`, skipping any the original lacks),
//   merging `orig.__dict__`, and setting `__wrapped__` — then returns
//   the wrapper.  UserFunction exposes mutable overrides for all these
//   (`user_name`/`user_qualname`/`module`/`doc`/`attrs`/`annotations`
//   in `pyrust-core`; see `env.rs::assign_attr_function`), so the
//   wrapper stays a real function (`type(w).__name__ == "function"`).
//   `update_wrapper(wrapper, wrapped)` is the same operation exposed
//   directly.
//
// Reference: <https://docs.python.org/3/library/functools.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::interpreter::{Interpreter, lookup_class_attr, object_class_singleton};
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, PyKey, StrKey, Value, ValueKind};
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
        let items = _interp.collect_iterable(&args[1].value)?;
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
            // First positional is `func`; remaining positional+keyword are
            // the pre-bound args/kwargs.  `func` itself is positional-only
            // in CPython (the `/` in the signature) so we don't accept
            // `func=` as a keyword.
            let func = match user.first() {
                Some(a) if a.name.is_none() => a.value.clone(),
                Some(a) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() got an unexpected keyword argument '{}' \
                             ('func' is positional-only)",
                            a.name.as_deref().unwrap_or(""),
                        ),
                    ));
                }
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME} expected at least 1 argument, got 0"),
                    ));
                }
            };
            let mut bound_args: Vec<Value> = Vec::new();
            let mut bound_kwargs: PyDict = PyDict::default();
            for a in &user[1..] {
                match &a.name {
                    Some(n) => {
                        bound_kwargs.insert(PyKey::str_from(n.as_str()), a.value.clone());
                    }
                    None => bound_args.push(a.value.clone()),
                }
            }
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("func", func);
            attrs
                .attrs
                .insert("args", Value::tuple(bound_args));
            attrs
                .attrs
                .insert("keywords", Value::dict(bound_kwargs));
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
                    kw_map.insert(name.as_str().unwrap_or("").to_owned(), v);
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

        /// `repr(partial(f, ...))` — `functools.partial(<func repr>, arg, kw=val)`.
        /// The embedded `func` repr carries an address in CPython
        /// (`<function f at 0x...>`).  Positional args and keyword values are
        /// each rendered via the interpreter-aware repr so user `__repr__`
        /// is honoured; keyword args keep their insertion order.
        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (func, bound_pos, bound_kw) = read_partial_state(&inst, FN_NAME)?;
            let mut parts: Vec<String> =
                Vec::with_capacity(1 + bound_pos.len() + bound_kw.len());
            parts.push(crate::builtin_modules::builtins::render_value_repr(_interp, &func)?);
            for v in &bound_pos {
                parts.push(crate::builtin_modules::builtins::render_value_repr(_interp, v)?);
            }
            for (k, v) in &bound_kw {
                let name = match k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => continue,
                };
                let val_repr =
                    crate::builtin_modules::builtins::render_value_repr(_interp, v)?;
                parts.push(format!("{name}={val_repr}"));
            }
            Ok(Value::string(format!("functools.partial({})", parts.join(", "))))
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
            reject_extra_args(args, FN_NAME)
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
            let key_pykey = PyKey::str_from(&key);
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
                bump_counter(&inst, "_hits");
                return Ok(v);
            }
            // Miss: compute, insert, evict if over capacity.
            bump_counter(&inst, "_misses");
            let result = _interp.call_function_expanded(func, user)?;
            insert_cache(&inst, key, key_pykey, result.clone(), maxsize);
            Ok(result)
        }

        /// `wrapper.cache_clear()` — drop all cached entries and reset the
        /// hit/miss counters.  Matches CPython's API.
        fn cache_clear(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let mut borrow = inst.borrow_mut();
            borrow
                .attrs
                .insert("_cache", Value::dict(PyDict::default()));
            borrow
                .attrs
                .insert("_order", Value::list(Vec::new()));
            borrow.attrs.insert("_hits", Value::int(0));
            borrow.attrs.insert("_misses", Value::int(0));
            let _ = _interp;
            Ok(Value::none())
        }

        /// `wrapper.cache_info()` — return a `CacheInfo(hits, misses,
        /// maxsize, currsize)` named-tuple.  `currsize` is the number of
        /// live cache entries; `maxsize` is the configured bound (`None`
        /// for unbounded / `functools.cache`).
        fn cache_info(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (hits, misses, maxsize, currsize) = {
                let borrow = inst.borrow();
                let hits = counter_value(&borrow.attrs, "_hits");
                let misses = counter_value(&borrow.attrs, "_misses");
                let maxsize = borrow
                    .attrs
                    .get("_maxsize")
                    .cloned()
                    .unwrap_or_else(Value::none);
                let currsize = borrow
                    .attrs
                    .get("_cache")
                    .and_then(|v| v.as_dict().map(|d| d.len()))
                    .unwrap_or(0) as i64;
                (hits, misses, maxsize, currsize)
            };
            make_cache_info(_interp, hits, misses, maxsize, currsize)
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
            reject_extra_args(args, FN_NAME)
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

    /// CPython: functools.update_wrapper(wrapper, wrapped,
    /// assigned=WRAPPER_ASSIGNMENTS, updated=WRAPPER_UPDATES).
    /// Mutates `wrapper` to look like `wrapped`: copies the
    /// WRAPPER_ASSIGNMENTS attrs (skipping any the wrapped object lacks),
    /// merges `wrapped.__dict__` into `wrapper.__dict__`, sets
    /// `wrapper.__wrapped__ = wrapped`, and returns `wrapper`.
    /// <https://docs.python.org/3/library/functools.html#functools.update_wrapper>
    fn update_wrapper(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        // CPython's `assigned` / `updated` parameters are rarely overridden;
        // we accept the common 2-argument form and use the standard
        // WRAPPER_ASSIGNMENTS / WRAPPER_UPDATES.
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 2 arguments ({} given)", args.len()),
            ));
        }
        let wrapper = args[0].value.clone();
        let wrapped = args[1].value.clone();
        do_update_wrapper(_interp, &wrapper, &wrapped)?;
        Ok(wrapper)
    }

    /// CPython: functools.wraps(wrapped).
    /// Returns a decorator that, applied to `wrapper`, runs
    /// `update_wrapper(wrapper, wrapped)` and returns the (mutated)
    /// wrapper.  Equivalent to `partial(update_wrapper, wrapped=wrapped)`.
    /// <https://docs.python.org/3/library/functools.html#functools.wraps>
    fn wraps(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let _ = _interp;
        Ok(make_wraps_partial(args[0].value.clone()))
    }

    /// `wraps(wrapped)` returns one of these.  Calling it with the
    /// wrapper function mutates that wrapper (copying `wrapped`'s
    /// metadata) and returns it — so the decorated name stays a real
    /// function.
    class _wraps_partial {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            // Private constructor — `wraps()` seeds attrs directly via
            // `make_wraps_partial`.  Reject user args so calling
            // `_wraps_partial(...)` from outside this module fails
            // loudly rather than producing a broken instance.
            reject_extra_args(args, FN_NAME)
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
            let wrapped = inst
                .borrow()
                .attrs
                .get("__wraps_func")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            let wrapper = user[0].value.clone();
            do_update_wrapper(_interp, &wrapper, &wrapped)?;
            Ok(wrapper)
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

    /// CPython: functools.cache(user_function).
    /// Simple lightweight unbounded function cache — exactly equivalent to
    /// `lru_cache(maxsize=None)`.  Bare-callable form only (CPython's `cache`
    /// takes a single positional function and no configuration).
    /// <https://docs.python.org/3/library/functools.html#functools.cache>
    fn cache(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let func = args[0].value.clone();
        let _ = _interp;
        // Unbounded memo == lru_cache with maxsize=None, typed=False.
        Ok(make_lru_wrapper(func, None, false))
    }

    /// CPython: functools.cmp_to_key(mycmp).
    /// Converts an old-style comparison function (`mycmp(a, b) -> int`, where
    /// the sign of the result orders `a` relative to `b`) into a key function
    /// for use with `sorted`, `min`, `max`, etc.  Returns a callable that wraps
    /// each value in a comparison object whose rich-comparison dunders call
    /// `mycmp` and test its result against zero.
    /// <https://docs.python.org/3/library/functools.html#functools.cmp_to_key>
    fn cmp_to_key(args) -> Result<Value> {
        if args.len() != 1 || args[0].name.is_some() {
            // CPython's C `cmp_to_key` is `cmp_to_key(mycmp)` — a single
            // positional argument.
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 1 argument, got {}", args.len()),
            ));
        }
        let _ = _interp;
        let mut attrs = InstanceAttrs::new();
        attrs.insert("_cmp", args[0].value.clone());
        Ok(make_instance("_cmp_to_key", attrs))
    }

    /// The callable returned by `cmp_to_key(mycmp)`.  Calling it with a value
    /// `obj` produces a `_cmp_key` object that wraps `obj` together with the
    /// comparison function.
    class _cmp_to_key {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            // Private constructor — `cmp_to_key()` seeds `_cmp` directly via
            // `make_instance`.  Reject user args so a stray
            // `_cmp_to_key(...)` call fails loudly.
            reject_extra_args(args, FN_NAME)
        }

        fn __call__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() != 1 || user[0].name.is_some() {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME} expected 1 argument, got {}", user.len()),
                ));
            }
            let cmp = inst
                .borrow()
                .attrs
                .get("_cmp")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            let _ = _interp;
            let mut attrs = InstanceAttrs::new();
            attrs.insert("obj", user[0].value.clone());
            attrs.insert("_cmp", cmp);
            Ok(make_instance("_cmp_key", attrs))
        }
    }

    /// The comparison wrapper produced by `cmp_to_key(mycmp)(obj)`.  Each rich
    /// comparison calls `mycmp(self.obj, other.obj)` and tests the sign of the
    /// result, mirroring CPython's `functools.K`.
    class _cmp_key {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            reject_extra_args(args, FN_NAME)
        }

        fn __lt__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Lt, FN_NAME)
        }

        fn __gt__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Gt, FN_NAME)
        }

        fn __eq__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Eq, FN_NAME)
        }

        fn __le__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Le, FN_NAME)
        }

        fn __ge__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Ge, FN_NAME)
        }
    }

    /// CPython: functools.total_ordering(cls).
    /// Class decorator that fills in the missing rich-comparison methods given
    /// at least one of `__lt__` / `__le__` / `__gt__` / `__ge__`.  Raises
    /// `ValueError` if no ordering operation is defined.  The derived methods
    /// are compiled from the same formulas CPython uses (see
    /// `derivation_source`).
    /// <https://docs.python.org/3/library/functools.html#functools.total_ordering>
    fn total_ordering(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let cls_val = args[0].value.clone();
        let class = match cls_val.kind() {
            ValueKind::PyClass(c) => c,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument must be a class"),
                ));
            }
        };
        apply_total_ordering(_interp, class)?;
        Ok(cls_val)
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
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Shared arity guard for this module's private no-op `__init__`s.
/// These classes are constructed internally with their attrs seeded
/// directly, so the `__init__` only needs to reject any user-provided
/// arguments (everything past the implicit `self`) and return `None`.
fn reject_extra_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments (got {})", args.len() - 1),
        ));
    }
    Ok(Value::none())
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
        ValueKind::PyClass(c) => Some(Rc::clone(c)),
        _ => None,
    }
}

/// Construct a `PyInstance` of class `name` with the supplied attrs,
/// bypassing `__init__`.  Used by the LRU and wraps helpers below to
/// seed private state without going through a public constructor.
fn make_instance(name: &str, attrs: InstanceAttrs) -> Value {
    match module_class(name) {
        Some(class) => {
            // CPython's `cmp_to_key` wrapper (`functools.K`) is unhashable: it
            // defines the rich-comparison dunders but no `__hash__`, and any
            // class that overrides `__eq__` without `__hash__` is unhashable
            // (the pure-Python version uses `__slots__`; the C version sets
            // `tp_hash = PyObject_HashNotImplemented`).  pyrust applies the
            // same `__eq__`-implies-unhashable rule to user `class` statements,
            // but `pyrust_module!` `class` blocks don't get the automatic
            // `__hash__ = None`, so graft it on (idempotently) here.
            if name == "_cmp_key"
                && !class.borrow().attrs.contains_key("__hash__")
            {
                class
                    .borrow_mut()
                    .attrs
                    .insert("__hash__".to_string(), Value::none());
            }
            Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class,
                attrs,
            })))
        }
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
) -> Result<(Value, Vec<Value>, PyDict)> {
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
    let order = inst.borrow().attrs.get("_order").cloned();
    if let Some(order) = order {
        order.list_with_mut(|items| {
            if let Some(pos) = items
                .iter()
                .position(|v| matches!(v.kind(), ValueKind::Str(s) if s == key))
            {
                let item = items.remove(pos);
                items.push(item);
            }
        });
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
    let order_val = inst.borrow().attrs.get("_order").cloned();
    let cache_val = inst.borrow().attrs.get("_cache").cloned();
    let order_full = if let Some(ref order) = order_val {
        order
            .list_with_mut(|items| {
                items.push(Value::string(&key));
                match maxsize {
                    Some(0) => true,
                    Some(n) => items.len() > n as usize,
                    None => false,
                }
            })
            .unwrap_or(false)
    } else {
        false
    };
    if let Some(ref cache) = cache_val {
        let _ = cache.dict_with_mut(|dict| {
            dict.insert(key_pykey, value);
        });
    }
    // Evict head if over capacity.  `maxsize=0` is a degenerate case
    // — no entries are kept; we immediately evict what we just
    // inserted.
    if order_full {
        let evict_key: Option<String> = order_val.as_ref().and_then(|order| {
            order.list_with_mut(|items| {
                if items.is_empty() {
                    None
                } else {
                    let head = items.remove(0);
                    match head.kind() {
                        ValueKind::Str(s) => Some(s.to_string()),
                        _ => None,
                    }
                }
            })?
        });
        if let Some(k) = evict_key
            && let Some(ref cache) = cache_val
        {
            let _ = cache.dict_with_mut(|dict| {
                dict.shift_remove(&StrKey(&k));
            });
        }
    }
}

/// Construct a `_lru_cache_wrapper` instance seeded with `func` /
/// `maxsize` / `typed`.
fn make_lru_wrapper(func: Value, maxsize: Option<i64>, typed: bool) -> Value {
    let mut attrs = InstanceAttrs::new();
    // `wrapper.__wrapped__` exposes the original function (CPython sets this
    // on the wrapper so `inspect.unwrap` / introspection can reach it).
    attrs.insert("__wrapped__", func.clone());
    attrs.insert("_func", func);
    attrs.insert(
        "_maxsize",
        maxsize.map_or_else(Value::none, Value::int),
    );
    attrs.insert("_typed", Value::bool_(typed));
    attrs.insert("_cache", Value::dict(PyDict::default()));
    attrs.insert("_order", Value::list(Vec::new()));
    attrs.insert("_hits", Value::int(0));
    attrs.insert("_misses", Value::int(0));
    make_instance("_lru_cache_wrapper", attrs)
}

/// Construct a `_lru_cache_factory` instance — the decorator returned
/// by `lru_cache(maxsize=N)` / `lru_cache()`.
fn make_lru_factory(maxsize: Option<i64>, typed: bool) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert(
        "_maxsize",
        maxsize.map_or_else(Value::none, Value::int),
    );
    attrs.insert("_typed", Value::bool_(typed));
    make_instance("_lru_cache_factory", attrs)
}

/// Increment the integer counter stored under `key` on the wrapper
/// instance (`_hits` / `_misses`).  Missing/non-int treated as 0.
fn bump_counter(inst: &Rc<RefCell<PyInstance>>, key: &str) {
    let mut borrow = inst.borrow_mut();
    let cur = match borrow.attrs.get(key).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        _ => 0,
    };
    borrow
        .attrs
        .insert(key, Value::int(cur.wrapping_add(1)));
}

/// Read an integer counter from the instance attrs, defaulting to 0.
fn counter_value(attrs: &InstanceAttrs, key: &str) -> i64 {
    match attrs.get(key).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        _ => 0,
    }
}

/// Build a `CacheInfo(hits, misses, maxsize, currsize)` named-tuple
/// instance.  `CacheInfo` is a `tuple` subclass (matching CPython:
/// `isinstance(info, tuple)` is `True`, fields are indexable, and the
/// repr is `CacheInfo(hits=.., misses=.., maxsize=.., currsize=..)`).
/// The class is defined once via interpreter `exec` and cached.
fn make_cache_info(
    interp: &mut Interpreter,
    hits: i64,
    misses: i64,
    maxsize: Value,
    currsize: i64,
) -> Result<Value> {
    let class = cache_info_class(interp)?;
    interp.call_function_expanded(
        class,
        &[
            ExpandedCallArg { name: None, value: Value::int(hits) },
            ExpandedCallArg { name: None, value: Value::int(misses) },
            ExpandedCallArg { name: None, value: maxsize },
            ExpandedCallArg { name: None, value: Value::int(currsize) },
        ],
    )
}

thread_local! {
    /// Cached `CacheInfo` class, built once per thread on first
    /// `cache_info()` call.
    static CACHE_INFO_CLASS: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// Lazily define and return the `CacheInfo` named-tuple class.
fn cache_info_class(interp: &mut Interpreter) -> Result<Value> {
    if let Some(cls) = CACHE_INFO_CLASS.with(|c| c.borrow().clone()) {
        return Ok(cls);
    }
    let ns = Value::dict(PyDict::default());
    interp.exec_source(CACHE_INFO_SOURCE, Some(ns.clone()), None)?;
    let cls = ns
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from("CacheInfo")).cloned())
        .ok_or_else(|| internal("cache_info"))?;
    CACHE_INFO_CLASS.with(|c| *c.borrow_mut() = Some(cls.clone()));
    Ok(cls)
}

/// Python source for the `CacheInfo` named-tuple, transcribed to match
/// CPython's `collections.namedtuple('CacheInfo', [...])` behaviour
/// (tuple subclass, indexable, attribute access, custom repr).
const CACHE_INFO_SOURCE: &str = "\
class CacheInfo(tuple):
    __slots__ = ()
    _fields = ('hits', 'misses', 'maxsize', 'currsize')
    def __new__(cls, hits, misses, maxsize, currsize):
        return tuple.__new__(cls, (hits, misses, maxsize, currsize))
    @classmethod
    def _make(cls, iterable):
        return cls(*iterable)
    def _asdict(self):
        return {f: self[i] for i, f in enumerate(self._fields)}
    def _replace(self, **kwds):
        vals = list(self)
        for i, f in enumerate(self._fields):
            if f in kwds:
                vals[i] = kwds.pop(f)
        if kwds:
            raise ValueError(f'Got unexpected field names: {list(kwds)!r}')
        return self.__class__(*vals)
    @property
    def hits(self):
        return self[0]
    @property
    def misses(self):
        return self[1]
    @property
    def maxsize(self):
        return self[2]
    @property
    def currsize(self):
        return self[3]
    def __repr__(self):
        return f'CacheInfo(hits={self[0]}, misses={self[1]}, maxsize={self[2]}, currsize={self[3]})'
";

// ── wraps helpers ────────────────────────────────────────────────────────────

fn make_wraps_partial(wrapped: Value) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("__wraps_func", wrapped);
    make_instance("_wraps_partial", attrs)
}

/// The attributes copied directly from `wrapped` to `wrapper`, mirroring
/// CPython's `functools.WRAPPER_ASSIGNMENTS`.  (CPython 3.12 also lists
/// `__type_params__`, but pyrust does not expose that attribute on
/// functions, so copying it would be a no-op on every supported type;
/// omitting it keeps the observable behaviour identical.)
const WRAPPER_ASSIGNMENTS: [&str; 5] =
    ["__module__", "__name__", "__qualname__", "__annotations__", "__doc__"];

/// Core of `update_wrapper` / `wraps`.  Mutates `wrapper` in place to
/// look like `wrapped`, mirroring CPython's pure-Python `update_wrapper`:
///   1. copy each WRAPPER_ASSIGNMENTS attr present on `wrapped`
///      (skipping any that raise `AttributeError`),
///   2. update `wrapper.__dict__` with `wrapped.__dict__`,
///   3. set `wrapper.__wrapped__ = wrapped` (last, so it isn't clobbered
///      by the `__dict__` merge).
fn do_update_wrapper(
    interp: &mut Interpreter,
    wrapper: &Value,
    wrapped: &Value,
) -> Result<()> {
    for attr in WRAPPER_ASSIGNMENTS {
        // CPython does `try: value = getattr(wrapped, attr) except
        // AttributeError: pass`.  Any *other* error propagates.
        match interp.get_attr(wrapped, attr) {
            Ok(value) => interp.assign_attr(wrapper.clone(), attr, value)?,
            // A missing attribute may surface as the structured
            // `AttributeError` variant *or* as `PyError::Named("AttributeError",
            // …)` (e.g. built-ins / type objects raise the latter), so match by
            // class name to cover both.
            Err(e) if e.class_name_is("AttributeError") => {}
            Err(e) => return Err(e),
        }
    }
    // `wrapper.__dict__.update(wrapped.__dict__)`.  CPython merges into the
    // wrapper's existing dict rather than replacing it.
    if let Ok(dst) = interp.get_attr(wrapper, "__dict__") {
        let src = match interp.get_attr(wrapped, "__dict__") {
            Ok(d) => d,
            Err(e) if e.class_name_is("AttributeError") => Value::dict(PyDict::default()),
            Err(e) => return Err(e),
        };
        if let (Some(src_dict), Some(_)) = (src.as_dict(), dst.as_dict()) {
            for (k, v) in src_dict.iter() {
                let k = k.clone();
                let v = v.clone();
                let _ = dst.dict_with_mut(|d| {
                    d.insert(k, v);
                });
            }
        }
    }
    // Set `__wrapped__` last (CPython issue #17482).
    interp.assign_attr(wrapper.clone(), "__wrapped__", wrapped.clone())
}

/// Best-effort `__name__` extractor for `cached_property`.
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

// ── cmp_to_key helpers ───────────────────────────────────────────────────────

/// Shared body for `_cmp_key.__lt__/__gt__/__eq__/__le__/__ge__`.
/// `args[0]` is `self` (a `_cmp_key`), `args[1]` is `other`.  Calls the wrapped
/// comparison function on the two `obj`s and tests its result against `0` with
/// `op` (e.g. `__lt__` → `cmp(self.obj, other.obj) < 0`), mirroring CPython's
/// `functools.K`.
fn cmp_key_compare(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    op: BinaryOp,
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name} expected 1 argument, got {}", user.len()),
        ));
    }
    // The right operand must also be a `_cmp_key` (as produced by the same
    // `cmp_to_key` call).  When it isn't, defer to Python's reflected-operand
    // machinery by returning `NotImplemented` — matching CPython, which only
    // ever compares two key objects during a sort.
    let other_obj = match user[0].value.kind() {
        ValueKind::PyInstance(other) => other.borrow().attrs.get("obj").cloned(),
        _ => None,
    };
    let Some(other_obj) = other_obj else {
        return Ok(Value::not_implemented());
    };
    let (cmp, self_obj) = {
        let borrow = inst.borrow();
        (
            borrow
                .attrs
                .get("_cmp")
                .cloned()
                .ok_or_else(|| internal(fn_name))?,
            borrow
                .attrs
                .get("obj")
                .cloned()
                .ok_or_else(|| internal(fn_name))?,
        )
    };
    let result = interp.call_function_expanded(
        cmp,
        &[
            ExpandedCallArg { name: None, value: self_obj },
            ExpandedCallArg { name: None, value: other_obj },
        ],
    )?;
    let cmp_bool = interp.eval_binary(result, op, Value::int(0))?;
    Ok(Value::bool_(cmp_bool.truthy()))
}

// ── total_ordering helpers ───────────────────────────────────────────────────

/// The four ordering operations in CPython's `max(roots)` priority order.
/// CPython picks `root = max(roots)` over the *string* names, and
/// `'__lt__' > '__le__' > '__gt__' > '__ge__'` lexicographically — so the
/// first present op in this list is the chosen root.
const ORDERING_OPS: [&str; 4] = ["__lt__", "__le__", "__gt__", "__ge__"];

/// Is `op` defined on `class` to something other than the inherited
/// `object.<op>` default?  Mirrors CPython's
/// `getattr(cls, op, None) is not getattr(object, op, None)`.
fn ordering_op_is_root(class: &Rc<RefCell<PyClass>>, op: &str) -> bool {
    match lookup_class_attr(class, op) {
        None => false,
        // Inherited straight from `object` (the builtin slot) → not a root.
        Some(v) => !matches!(
            v.kind(),
            ValueKind::BuiltinFunction(n) if n == format!("object.{op}")
        ),
    }
}

/// Apply `@total_ordering` to `class`: derive the missing ordering operators
/// from the highest-priority present one, compiling the derived methods from
/// the same formulas CPython uses.
fn apply_total_ordering(
    interp: &mut Interpreter,
    class: &Rc<RefCell<PyClass>>,
) -> Result<()> {
    // Guard against the bare `object` singleton — its ordering slots are the
    // builtin defaults, so it would never count as a root anyway, but a stray
    // `total_ordering(object)` shouldn't mutate the shared singleton.
    if Rc::ptr_eq(class, &object_class_singleton()) {
        return Err(PyError::named(
            "ValueError",
            "must define at least one ordering operation: < > <= >=".to_string(),
        ));
    }
    // `root = max(roots)` over the op names; ORDERING_OPS is in descending
    // string order, so the first present op is the chosen root.
    let root = ORDERING_OPS
        .into_iter()
        .find(|op| ordering_op_is_root(class, op));
    let Some(root) = root else {
        return Err(PyError::named(
            "ValueError",
            "must define at least one ordering operation: < > <= >=".to_string(),
        ));
    };
    // Compile the derivation functions once into a fresh namespace, then graft
    // the ones not already defined directly onto the class (CPython's
    // `setattr(cls, opname, opfunc)`).
    let source = derivation_source(root);
    let ns = Value::dict(PyDict::default());
    interp.exec_source(source, Some(ns.clone()), None)?;
    for (opname, _) in convert_table(root) {
        if !ordering_op_is_root(class, opname) {
            let func = ns
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(opname)).cloned())
                .ok_or_else(|| internal("total_ordering"))?;
            class.borrow_mut().attrs.insert(opname.to_string(), func);
        }
    }
    class.borrow().mutation_version.set(
        class.borrow().mutation_version.get().wrapping_add(1),
    );
    Ok(())
}

/// CPython's `_convert[root]` — the ordered list of `(opname, derived-from)`
/// pairs for a given root operator.
fn convert_table(root: &str) -> &'static [(&'static str, &'static str)] {
    match root {
        "__lt__" => &[
            ("__gt__", "__lt__"),
            ("__le__", "__lt__"),
            ("__ge__", "__lt__"),
        ],
        "__le__" => &[
            ("__ge__", "__le__"),
            ("__lt__", "__le__"),
            ("__gt__", "__le__"),
        ],
        "__gt__" => &[
            ("__lt__", "__gt__"),
            ("__ge__", "__gt__"),
            ("__le__", "__gt__"),
        ],
        // "__ge__"
        _ => &[
            ("__le__", "__ge__"),
            ("__gt__", "__ge__"),
            ("__lt__", "__ge__"),
        ],
    }
}

/// Python source defining the three derived ordering methods for `root`,
/// transcribed from CPython's `functools._<op>_from_<root>` helpers (the
/// `NotImplemented` short-circuit included).  Executing this in a fresh
/// namespace yields `UserFunction` values keyed by op name.
fn derivation_source(root: &str) -> &'static str {
    match root {
        "__lt__" => "\
def __gt__(self, other):
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result and self != other
def __le__(self, other):
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result or self == other
def __ge__(self, other):
    op_result = type(self).__lt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
",
        "__le__" => "\
def __ge__(self, other):
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result or self == other
def __lt__(self, other):
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result and self != other
def __gt__(self, other):
    op_result = type(self).__le__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
",
        "__gt__" => "\
def __lt__(self, other):
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result and self != other
def __ge__(self, other):
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result or self == other
def __le__(self, other):
    op_result = type(self).__gt__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
",
        // "__ge__"
        _ => "\
def __le__(self, other):
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result or self == other
def __gt__(self, other):
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return op_result and self != other
def __lt__(self, other):
    op_result = type(self).__ge__(self, other)
    if op_result is NotImplemented:
        return op_result
    return not op_result
",
    }
}
