// `functools` module — body for the `functools` entry in
// `pyrust_builtin_modules!`. Owns the concrete functools APIs (`reduce`,
// `partial`, caching decorators, wrapper metadata helpers, `cmp_to_key`,
// `singledispatch`, and `total_ordering`).
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
//   `property` keeps the hot attribute-lookup path tight; see the runtime
//   environment descriptor domain.
//
// - `wraps(orig)` returns a `_wraps_partial` callable.  Calling it with
//   the wrapper function mutates that wrapper in place — copying the
//   WRAPPER_ASSIGNMENTS attrs (`__module__`/`__name__`/`__qualname__`/
//   `__annotations__`/`__doc__`, skipping any the original lacks),
//   merging `orig.__dict__`, and setting `__wrapped__` — then returns
//   the wrapper.  UserFunction exposes mutable overrides for all these
//   (`user_name`/`user_qualname`/`module`/`doc`/`attrs`/`annotations`
//   in `pyrust-core`), so the
//   wrapper stays a real function (`type(w).__name__ == "function"`).
//   `update_wrapper(wrapper, wrapped)` is the same operation exposed
//   directly.
//
// Reference: <https://docs.python.org/3/library/functools.html>

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::interpreter::{Interpreter, lookup_class_attr, object_class_singleton, value_class};
use crate::value::{
    InstanceAttrs, ModuleMutationState, PyClass, PyDict, PyInstance, PyKey, PyModule, Value,
    ValueKind,
};
use indexmap::IndexMap;
use pyrust_core::BuiltinTypeOps;
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
        let iterator = crate::interpreter::make_iterator(_interp, &args[1].value)?;
        let mut acc = if args.len() == 3 {
            // Initializer supplied → it's the seed; the iterable is folded
            // onto it from the left.  This is the only branch that's
            // happy with an empty iterable.
            args[2].value.clone()
        } else {
            // No initializer → first item is the seed.  Empty iterable
            // is a hard error (mirrors CPython's TypeError text).
            match _interp.call_next(&iterator, None) {
                Ok(value) => value,
                Err(ref error) if crate::interpreter::is_stop_iteration_error(error) => {
                    return Err(PyError::named(
                    "TypeError",
                    // CPython's C `reduce` uses the bare `reduce()` here, with
                    // no `functools.` module prefix (unlike most other arity
                    // errors in this module, which carry the qualified name).
                        "reduce() of empty iterable with no initial value".to_string(),
                    ))
                }
                Err(error) => return Err(error),
            }
        };
        loop {
            let item = match _interp.call_next(&iterator, None) {
                Ok(value) => value,
                Err(ref error) if crate::interpreter::is_stop_iteration_error(error) => break,
                Err(error) => return Err(error),
            };
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
            parts.push(crate::interpreter::render_value_repr(_interp, &func)?);
            for v in &bound_pos {
                parts.push(crate::interpreter::render_value_repr(_interp, v)?);
            }
            for (k, v) in &bound_kw {
                let name = match k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => continue,
                };
                let val_repr = crate::interpreter::render_value_repr(_interp, v)?;
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
        let (generation, args) = split_generation_arg(args, FN_NAME)?;
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
            let (wrapper_class, cache_info_class) = lru_wrapper_classes(_interp, generation)?;
            return Ok(make_lru_wrapper(
                wrapper_class,
                cache_info_class,
                positional[0].value.clone(),
                Some(128),
                false,
            ));
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
        let factory_class = lru_factory_class(_interp, generation)?;
        Ok(make_lru_factory(
            generation,
            factory_class,
            maxsize,
            typed,
        ))
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

            // CPython's maxsize=0 wrapper is a statistics-only pass-through:
            // it does not hash the arguments (so even an unhashable list is
            // accepted), and every call is a miss.
            if maxsize == Some(0) {
                bump_counter(&inst, "_misses");
                return _interp.call_function_expanded(func, user);
            }

            let key = build_key(_interp, user, typed)?;
            let cache = lru_cache_value(&inst, FN_NAME)?;
            if let Some((_index, entry)) = _interp.dict_lookup(&cache, &key)? {
                let (value, node_id) = decode_cache_entry(entry, maxsize.is_some(), FN_NAME)?;
                // A bounded cache of size one has only one live node, which is
                // necessarily already the MRU node. Avoid cloning/downcasting
                // the private link state for this common steady-hit case.
                if maxsize != Some(1)
                    && let Some(node_id) = node_id
                {
                    with_lru_links(&inst, FN_NAME, |links| links.promote(node_id))?;
                }
                bump_counter(&inst, "_hits");
                return Ok(value);
            }

            // Miss: compute, insert, evict if over capacity.
            bump_counter(&inst, "_misses");
            let result = _interp.call_function_expanded(func, user)?;
            insert_cache(_interp, &inst, key, result.clone(), maxsize, FN_NAME)?;
            Ok(result)
        }

        /// `wrapper.cache_clear()` — drop all cached entries and reset the
        /// hit/miss counters.  Matches CPython's API.
        fn cache_clear(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            {
                let mut borrow = inst.borrow_mut();
                borrow
                    .attrs
                    .insert("_cache", Value::dict(PyDict::default()));
                borrow.attrs.insert("_hits", Value::int(0));
                borrow.attrs.insert("_misses", Value::int(0));
            }
            with_lru_links(&inst, FN_NAME, LruLinks::clear)?;
            let _ = _interp;
            Ok(Value::none())
        }

        /// `wrapper.cache_info()` — return a `CacheInfo(hits, misses,
        /// maxsize, currsize)` named-tuple.  `currsize` is the number of
        /// live cache entries; `maxsize` is the configured bound (`None`
        /// for unbounded / `functools.cache`).
        fn cache_info(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (class, hits, misses, maxsize, currsize) = {
                let borrow = inst.borrow();
                let class = borrow
                    .attrs
                    .get("_cache_info_class")
                    .cloned()
                    .ok_or_else(|| internal(FN_NAME))?;
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
                (class, hits, misses, maxsize, currsize)
            };
            make_cache_info(_interp, class, hits, misses, maxsize, currsize)
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
            let (generation, maxsize, typed) = {
                let borrow = inst.borrow();
                let generation = borrow
                    .attrs
                    .get("_generation")
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
                (generation, maxsize, typed)
            };
            let (wrapper_class, cache_info_class) =
                lru_wrapper_classes(_interp, &generation)?;
            Ok(make_lru_wrapper(
                wrapper_class,
                cache_info_class,
                user[0].value.clone(),
                maxsize,
                typed,
            ))
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
        let (generation, args) = split_generation_arg(args, FN_NAME)?;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument"),
            ));
        }
        let _ = _interp;
        make_wraps_partial(generation, args[0].value.clone())
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
        let (generation, args) = split_generation_arg(args, FN_NAME)?;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let func = args[0].value.clone();
        // Unbounded memo == lru_cache with maxsize=None, typed=False.
        let (wrapper_class, cache_info_class) = lru_wrapper_classes(_interp, generation)?;
        Ok(make_lru_wrapper(
            wrapper_class,
            cache_info_class,
            func,
            None,
            false,
        ))
    }

    /// CPython: functools.cmp_to_key(mycmp).
    /// Converts an old-style comparison function (`mycmp(a, b) -> int`, where
    /// the sign of the result orders `a` relative to `b`) into a key function
    /// for use with `sorted`, `min`, `max`, etc.  Returns a callable that wraps
    /// each value in a comparison object whose rich-comparison dunders call
    /// `mycmp` and test its result against zero.
    /// <https://docs.python.org/3/library/functools.html#functools.cmp_to_key>
    fn cmp_to_key(args) -> Result<Value> {
        let (generation, args) = split_generation_arg(args, FN_NAME)?;
        if args.len() != 1 || args[0].name.is_some() {
            // CPython's C `cmp_to_key` is `cmp_to_key(mycmp)` — a single
            // positional argument.
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 1 argument, got {}", args.len()),
            ));
        }
        let mut attrs = InstanceAttrs::new();
        attrs.insert("_cmp", args[0].value.clone());
        attrs.insert(
            "_item_class",
            Value::py_class(generation_class(generation, "_cmp_key")?),
        );
        make_instance(generation, "_cmp_to_key", attrs)
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
            let (cmp, item_class) = {
                let borrow = inst.borrow();
                (
                    borrow
                        .attrs
                        .get("_cmp")
                        .cloned()
                        .ok_or_else(|| internal(FN_NAME))?,
                    borrow
                        .attrs
                        .get("_item_class")
                        .cloned()
                        .ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let ValueKind::PyClass(item_class) = item_class.kind() else {
                return Err(internal(FN_NAME));
            };
            let mut attrs = InstanceAttrs::new();
            attrs.insert("obj", user[0].value.clone());
            attrs.insert("_cmp", cmp);
            Ok(make_instance_with_class(
                Rc::clone(item_class),
                "_cmp_key",
                attrs,
            ))
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

        fn __ne__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Ne, FN_NAME)
        }

        fn __le__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Le, FN_NAME)
        }

        fn __ge__(args) -> Result<Value> {
            cmp_key_compare(_interp, args, BinaryOp::Ge, FN_NAME)
        }
    }

    /// CPython: functools.singledispatch(func).
    /// Transforms `func` into a single-dispatch generic function: the
    /// implementation chosen at call time is selected by the type of the
    /// first positional argument.  The returned wrapper exposes
    /// `.register(cls[, func])` (also usable as `@wrapper.register` with a
    /// type-annotated first parameter, or `@wrapper.register(cls)`),
    /// `.dispatch(cls)`, and a read-only-ish `.registry` mapping.
    ///
    /// The implementation is pure Python (executed lazily in a private
    /// namespace, like the `CacheInfo` named-tuple and `total_ordering`'s
    /// derivations).  It mirrors CPython's structure for the common case
    /// of concrete-class dispatch via `cls.__mro__`; it does not implement
    /// ABC virtual-subclass resolution (`_compose_mro`).
    /// <https://docs.python.org/3/library/functools.html#functools.singledispatch>
    fn singledispatch(args) -> Result<Value> {
        let (generation, args) = split_generation_arg(args, FN_NAME)?;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let func = args[0].value.clone();
        if !is_callable(&func) {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} requires a callable argument"),
            ));
        }
        let factory = singledispatch_factory(_interp, generation)?;
        _interp.call_function_expanded(
            factory,
            &[ExpandedCallArg { name: None, value: func }],
        )
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

// Non-macro responsibilities are kept in focused implementation fragments.
include!("functools/common.inc.rs");
include!("functools/class_registry.inc.rs");
include!("functools/partial.inc.rs");
include!("functools/lru_key.inc.rs");
include!("functools/lru_links.inc.rs");
include!("functools/lru_cache.inc.rs");
include!("functools/dynamic_factories.inc.rs");
include!("functools/wrapper_metadata.inc.rs");
include!("functools/cmp_to_key.inc.rs");
include!("functools/total_ordering.inc.rs");
