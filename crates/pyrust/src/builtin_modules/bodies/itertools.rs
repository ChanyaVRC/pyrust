// `itertools` module — body for the `itertools` entry in
// `pyrust_builtin_modules!`.  Exposes: `chain`, `islice`,
// `count`, `repeat`, `cycle`, `takewhile`, `dropwhile`, `starmap`,
// `accumulate`, `product`, `combinations`, `combinations_with_replacement`,
// `permutations`, `groupby`, `compress`, `zip_longest`, `filterfalse`,
// `tee`, `pairwise`, `batched`.
//
// ## Laziness
//
// Every iterator type defined here is a class with
// `__iter__` / `__next__` dunders that drive their source one element
// at a time via `Interpreter::call_next`.  Memory-bounded combinatorics
// (`product`, `combinations`, `permutations`, `combinations_with_replacement`)
// eagerly materialise their pool(s) into a Vec on `__init__`, because
// CPython's algorithms walk the pool by index repeatedly — that's
// equivalent to one materialisation, not "drain the source per yield".
//
// `chain` is lazy both across sources and within each source; it does not call
// `iter()` for a later source until the previous source is exhausted.
//
// Reference: <https://docs.python.org/3/library/itertools.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

#[path = "itertools/combinatoric_cursors.rs"]
mod combinatoric_cursors;
#[path = "itertools/native_iterators.rs"]
pub(crate) mod native_iterators;

pyrust_module! {
    /// CPython: itertools.chain(*iterables) — concatenate iterables.
    /// Lazy across sources (each is iterated only when reached).  A real
    /// `PyClass` so `type(chain(...))` is `<class 'itertools.chain'>` and
    /// `isinstance` / `chain.from_iterable` flow through the standard class
    /// machinery (#2370).
    /// <https://docs.python.org/3/library/itertools.html#itertools.chain>
    class chain {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("chain", user)?;
            // Keep the original sources untouched.  `__next__` calls `iter()`
            // only when it reaches that source, matching CPython's observable
            // timing: constructing a chain must not run `__iter__` on sources
            // that may never be consumed.
            let sources: Vec<Value> = user.iter().map(|a| a.value.clone()).collect();
            let mut a = inst.borrow_mut();
            a.attrs.insert("_sources", Value::list(sources));
            a.attrs.insert("_src_idx", Value::int(0));
            // `_cur_iter` holds the iterator for the source at `_src_idx`,
            // or None when none has been created yet for that index.
            a.attrs.insert("_cur_iter", Value::none());
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            loop {
                let (cur_iter, idx, n_sources) = {
                    let a = inst.borrow();
                    let cur_iter = a
                        .attrs
                        .get("_cur_iter")
                        .cloned()
                        .ok_or_else(|| internal(FN_NAME))?;
                    let idx = match a.attrs.get("_src_idx").map(|v| v.kind()) {
                        Some(ValueKind::Int(n)) => n as usize,
                        _ => return Err(internal(FN_NAME)),
                    };
                    let n_sources = match a.attrs.get("_sources").map(|v| v.kind()) {
                        Some(ValueKind::List(items)) => items.len(),
                        _ => return Err(internal(FN_NAME)),
                    };
                    (cur_iter, idx, n_sources)
                };
                if cur_iter.is_none() {
                    // No live iterator for the current source slot.
                    if idx >= n_sources {
                        return Err(PyError::named("StopIteration", String::new()));
                    }
                    // Lazily `iter()` the source at `idx` (matches CPython's
                    // per-source `GetIter` timing).
                    let source = {
                        let a = inst.borrow();
                        match a.attrs.get("_sources").map(|v| v.kind()) {
                            Some(ValueKind::List(items)) => items[idx].clone(),
                            _ => return Err(internal(FN_NAME)),
                        }
                    };
                    let iter = make_iter(_interp, &source)?;
                    inst.borrow_mut().attrs.insert("_cur_iter", iter);
                    continue;
                }
                match _interp.call_next(&cur_iter, None) {
                    Ok(v) => return Ok(v),
                    Err(e) if is_stop_iteration(&e) => {
                        // Advance to the next source; clear the live iterator.
                        let mut a = inst.borrow_mut();
                        a.attrs.insert("_src_idx", Value::int(idx as i64 + 1));
                        a.attrs.insert("_cur_iter", Value::none());
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        /// CPython: itertools.chain.from_iterable(iterable) — the alternate
        /// constructor that takes a *single* iterable whose elements are the
        /// iterables to chain.  Equivalent to `chain(*iterable)` but lazy: the
        /// outer iterable is consumed one element at a time, and each inner
        /// iterable is only iterated when reached, so an infinite outer source
        /// works up to the consumed point.  Exposed on the `chain` class, so
        /// `type(chain.from_iterable(...))` reports `<class 'itertools.chain'>`.
        /// <https://docs.python.org/3/library/itertools.html#itertools.chain.from_iterable>
        fn from_iterable(args) -> Result<Value> {
            // Module finalization installs this registry function as a native
            // classmethod descriptor. Its first argument is therefore the
            // concrete owner class: the current module generation, an older
            // retained generation, or a subclass. Keep that exact identity on
            // the native iterator state. The fallback covers direct internal
            // registry calls before a module has been finalized.
            let (class, user): (_, &[ExpandedCallArg]) = match args.split_first() {
                Some((receiver, user)) => match receiver.value.kind() {
                    ValueKind::PyClass(class) => (Some(Rc::clone(class)), user),
                    _ => (crate::interpreter::itertools_chain_class(), args),
                },
                None => (crate::interpreter::itertools_chain_class(), args),
            };
            reject_keyword_args_expanded("chain.from_iterable", user)?;
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "chain.from_iterable() takes exactly one argument ({} given)",
                        user.len()
                    ),
                ));
            }
            // `iter(arg)` over the outer iterable.  This does not consume any
            // element yet (for a generator source it just returns the generator);
            // the first element is pulled on the first `__next__`, matching
            // CPython's lazy timing. The module-owned cursor is exposed to the
            // generic iterator machinery only through its typed provider
            // advance interface, so there is no PyInstance `__next__` VM
            // re-entry per element (#2362).
            let outer = crate::interpreter::make_iterator(_interp, &user[0].value)?;
            Ok(native_iterators::chain_from_outer(class, outer))
        }
    }

    /// `itertools.islice` — fully lazy slice with class-based dispatch.
    /// State: `_iter` (advanced past start), `_remaining_stop` (Optional
    /// remaining count until stop), `_step`.
    /// <https://docs.python.org/3/library/itertools.html#itertools.islice>
    class islice {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("islice", user)?;
            if user.is_empty() || user.len() > 4 {
                let msg = if user.len() > 4 {
                    format!("islice expected at most 4 arguments, got {}", user.len())
                } else {
                    format!("islice expected at least 2 arguments, got {}", user.len())
                };
                return Err(PyError::named("TypeError", msg));
            }
            let (start, stop, step) = match user.len() {
                1 => return Err(PyError::named(
                    "TypeError",
                    "islice expected at least 2 arguments, got 1".to_string(),
                )),
                2 => (0i64, slice_arg(_interp, FN_NAME, &user[1].value, "stop")?, 1i64),
                3 => (
                    slice_arg(_interp, FN_NAME, &user[1].value, "start")?.unwrap_or(0),
                    slice_arg(_interp, FN_NAME, &user[2].value, "stop")?,
                    1i64,
                ),
                4 => (
                    slice_arg(_interp, FN_NAME, &user[1].value, "start")?.unwrap_or(0),
                    slice_arg(_interp, FN_NAME, &user[2].value, "stop")?,
                    slice_arg(_interp, FN_NAME, &user[3].value, "step")?.unwrap_or(1),
                ),
                _ => unreachable!("guarded above"),
            };
            if stop.is_some_and(|s| s < 0) || start < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize."
                        .to_string(),
                ));
            }
            if step <= 0 {
                return Err(PyError::named(
                    "ValueError",
                    "Step for islice() must be a positive integer or None.".to_string(),
                ));
            }
            let iter = make_iter(_interp, &user[0].value)?;
            let remaining: Option<i64> = stop.map(|s| (s - start).max(0));
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter", iter);
            // CPython creates the source iterator at construction, but does
            // not advance it until the first __next__. Even an empty slice
            // with start > stop consumes `start` items when first driven.
            a.attrs.insert("_skip", Value::int(start));
            a.attrs.insert("_initial_skip", Value::bool_(true));
            a.attrs.insert("_exhausted", Value::bool_(false));
            a.attrs.insert(
                "_remaining_stop",
                remaining.map_or_else(Value::none, Value::int),
            );
            a.attrs.insert("_step", Value::int(step));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            check_not_exhausted(&inst, "_exhausted")?;
            let (iter, mut skip, mut remaining, step) =
                read_islice_state(&inst, FN_NAME)?;
            // Skipping belongs to the *next* requested result.  In
            // particular, after yielding index 0 from islice(src, 0, None,
            // 3), CPython has consumed only that one source item; indices 1
            // and 2 are not pulled until the following __next__ call.
            //
            // For a bounded slice, never discard beyond `stop`.  The initial
            // `start` skip is deliberately left unbounded by `remaining`:
            // CPython consumes `start` items when a start >= stop slice is
            // first driven.
            let initial_skip = matches!(
                inst.borrow().attrs.get("_initial_skip").map(|v| v.kind()),
                Some(ValueKind::Bool(true))
            );
            let skip_now = if initial_skip {
                skip
            } else {
                remaining.map_or(skip, |r| skip.min(r))
            };
            let target_skip = skip - skip_now;
            while skip > target_skip {
                match _interp.call_next(&iter, None) {
                    Ok(_) => {
                        skip -= 1;
                        if !initial_skip {
                            remaining = remaining.map(|r| r - 1);
                        }
                    }
                    Err(e) => {
                        // CPython clears islice's source pointer on any
                        // iterator failure, including a non-StopIteration
                        // exception.  The original error is reported once;
                        // subsequent next() calls are exhausted.
                        inst.borrow_mut()
                            .attrs
                            .insert("_exhausted", Value::bool_(true));
                        return Err(e);
                    }
                }
            }
            {
                let mut attrs = inst.borrow_mut();
                attrs.attrs.insert("_skip", Value::int(0));
                attrs.attrs.insert("_initial_skip", Value::bool_(false));
            }
            if let Some(r) = remaining
                && r <= 0
            {
                inst.borrow_mut()
                    .attrs
                    .insert("_exhausted", Value::bool_(true));
                return Err(PyError::named("StopIteration", String::new()));
            }
            let item = match _interp.call_next(&iter, None) {
                Ok(item) => item,
                Err(error) => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_exhausted", Value::bool_(true));
                    return Err(error);
                }
            };
            remaining = remaining.map(|r| r - 1);
            // Defer step-1 discards until the next __next__ call.  Besides
            // preserving laziness, this also means an abandoned islice does
            // not advance its source beyond the last value actually yielded.
            {
                let mut attrs = inst.borrow_mut();
                attrs.attrs.insert("_skip", Value::int(step - 1));
                attrs.attrs.insert(
                    "_remaining_stop",
                    remaining.map_or_else(Value::none, Value::int),
                );
            }
            Ok(item)
        }
    }

    /// CPython: itertools.count(start=0, step=1) — infinite arithmetic
    /// progression.  Both args may be int or float (no float-step
    /// rounding-error workaround; CPython has the same drift).
    /// <https://docs.python.org/3/library/itertools.html#itertools.count>
    class count {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("count", user)?;
            if user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("count() takes at most 2 arguments ({} given)", user.len()),
                ));
            }
            let start = user
                .first()
                .cloned()
                .map(|a| a.value)
                .unwrap_or_else(|| Value::int(0));
            let step = user
                .get(1)
                .cloned()
                .map(|a| a.value)
                .unwrap_or_else(|| Value::int(1));
            require_numeric(&start, FN_NAME, "start")?;
            require_numeric(&step, FN_NAME, "step")?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_cur", start);
            a.attrs.insert("_step", step);
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (cur, step) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_cur").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_step").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let next = _interp.eval_binary(cur.clone(), crate::ast::BinaryOp::Add, step)?;
            inst.borrow_mut().attrs.insert("_cur", next);
            Ok(cur)
        }

        /// `repr(count(...))` — `count(start)` or `count(start, step)`.
        /// CPython omits the step when it is the default integer `1`
        /// (a float `1.0` step is always shown).  No address (CPython's
        /// `count_repr` formats only the numeric fields).
        fn __repr__(args) -> Result<Value> {
            let _ = _interp;
            let inst = expect_self(args, FN_NAME)?;
            let a = inst.borrow();
            let cur = a.attrs.get("_cur").cloned().ok_or_else(|| internal(FN_NAME))?;
            let step = a.attrs.get("_step").cloned().ok_or_else(|| internal(FN_NAME))?;
            // CPython's count_repr omits the step only when it is an *integer*
            // (incl. bool, a PyLong subclass) equal to 1.  A float `1.0` is
            // always shown.
            let omit_step =
                matches!(step.kind(), ValueKind::Int(1) | ValueKind::Bool(true));
            if omit_step {
                Ok(Value::string(format!("count({})", cur.repr_raw())))
            } else {
                Ok(Value::string(format!("count({}, {})", cur.repr_raw(), step.repr_raw())))
            }
        }
    }

    /// CPython: itertools.repeat(object[, times]) — yield `object`
    /// `times` times, or forever if `times` is *omitted*.  An explicit
    /// `None` is rejected as `TypeError` (matching CPython — `None`
    /// isn't an integer).
    /// <https://docs.python.org/3/library/itertools.html#itertools.repeat>
    class repeat {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("repeat", user)?;
            if user.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "repeat() missing required argument 'object' (pos 1)".to_string(),
                ));
            }
            if user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("repeat() takes at most 2 arguments ({} given)", user.len()),
                ));
            }
            let object = user[0].value.clone();
            // CPython 3.12: the `times` count honors the `__index__` protocol
            // (an int-subclass or `__index__` object is accepted), and a bigint
            // that overflows Py_ssize_t raises OverflowError (#2022).
            let times: Option<i64> = match user.get(1) {
                None => None,
                Some(a) => Some(_interp.value_to_isize(
                    &a.value,
                    "Python int too large to convert to C ssize_t",
                )?),
            };
            let mut a = inst.borrow_mut();
            a.attrs.insert("_object", object);
            a.attrs.insert(
                "_remaining",
                // Negative or zero `times` yields nothing (CPython behaviour).
                times.map_or_else(Value::none, |t| Value::int(t.max(0))),
            );
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (object, remaining) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_object").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_remaining").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            match remaining.kind() {
                ValueKind::None => Ok(object),
                ValueKind::Int(n) if n > 0 => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_remaining", Value::int(n - 1));
                    Ok(object)
                }
                _ => Err(PyError::named("StopIteration", String::new())),
            }
        }

        /// `repr(repeat(...))` — `repeat(obj)` for an unbounded repeat, or
        /// `repeat(obj, n)` when a `times` limit was given.  The count shown
        /// is the *remaining* number of items (matches CPython, which stores
        /// and decrements `cnt`).  No address.
        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (object, remaining) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_object").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_remaining").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let obj_repr = crate::interpreter::render_value_repr(_interp, &object)?;
            match remaining.kind() {
                ValueKind::Int(n) => {
                    Ok(Value::string(format!("repeat({obj_repr}, {n})")))
                }
                _ => Ok(Value::string(format!("repeat({obj_repr})"))),
            }
        }
    }

    /// CPython: itertools.cycle(iterable) — yield iterable's elements,
    /// then repeat forever from a remembered copy.
    /// <https://docs.python.org/3/library/itertools.html#itertools.cycle>
    class cycle {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("cycle", user)?;
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("cycle expected 1 argument, got {}", user.len()),
                ));
            }
            let iter = make_iter(_interp, &user[0].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter", iter);
            // `_cache` accumulates each yielded element during the first
            // pass; once `_iter` exhausts, we walk `_cache` indefinitely.
            a.attrs.insert("_cache", Value::list(Vec::new()));
            a.attrs.insert("_pos", Value::int(0));
            a.attrs.insert("_first_pass", Value::bool_(true));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let first_pass = matches!(
                inst.borrow().attrs.get("_first_pass").map(|v| v.kind()),
                Some(ValueKind::Bool(true))
            );
            if first_pass {
                let iter = inst
                    .borrow()
                    .attrs
                    .get("_iter")
                    .cloned()
                    .ok_or_else(|| internal(FN_NAME))?;
                match _interp.call_next(&iter, None) {
                    Ok(v) => {
                        // Append in place via the scoped `list_push`
                        // operation method (#448) so first-pass cost is
                        // amortised O(1) per element instead of O(n) per
                        // yield (the old code re-built the cache Vec from
                        // a slice clone).
                        let cache = inst
                            .borrow()
                            .attrs
                            .get("_cache")
                            .cloned()
                            .ok_or_else(|| internal(FN_NAME))?;
                        cache.list_push(v.clone())?;
                        return Ok(v);
                    }
                    Err(e) if is_stop_iteration(&e) => {
                        // First pass ended; switch over to walking _cache.
                        inst.borrow_mut()
                            .attrs
                            .insert("_first_pass", Value::bool_(false));
                        // Fall through to the cached-walk path below.
                    }
                    Err(e) => return Err(e),
                }
            }
            // Cached walk — read one element by index without cloning the
            // entire cache.  The old version did `items.to_vec()` per
            // yield, making each cached step O(n) on the cache length.
            let (item, cache_len, pos) = {
                let attrs = inst.borrow();
                let cache_val = attrs
                    .attrs
                    .get("_cache")
                    .ok_or_else(|| internal(FN_NAME))?;
                let pos = match attrs.attrs.get("_pos").map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => n,
                    _ => return Err(internal(FN_NAME)),
                };
                match cache_val.kind() {
                    ValueKind::List(items) if !items.is_empty() => {
                        let idx = (pos as usize) % items.len();
                        (items[idx].clone(), items.len(), pos)
                    }
                    ValueKind::List(_) => {
                        // Source was empty — nothing to cycle through.
                        return Err(PyError::named("StopIteration", String::new()));
                    }
                    _ => return Err(internal(FN_NAME)),
                }
            };
            inst.borrow_mut()
                .attrs
                .insert("_pos", Value::int((pos + 1) % cache_len as i64));
            Ok(item)
        }
    }

    /// CPython: itertools.takewhile(predicate, iterable) — yield while
    /// `predicate(x)` is truthy; stop at the first falsy result.
    /// <https://docs.python.org/3/library/itertools.html#itertools.takewhile>
    class takewhile {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("takewhile", user)?;
            if user.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("takewhile expected 2 arguments, got {}", user.len()),
                ));
            }
            let iter = make_iter(_interp, &user[1].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_pred", user[0].value.clone());
            a.attrs.insert("_iter", iter);
            a.attrs.insert("_done", Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            check_not_exhausted(&inst, "_done")?;
            let (pred, iter) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_pred").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let item = _interp.call_next(&iter, None)?;
            let verdict = _interp.call_function_expanded(
                pred,
                &[ExpandedCallArg {
                    name: None,
                    value: item.clone(),
                }],
            )?;
            // `truthy_value` dispatches `__bool__`/`__len__` on PyInstance
            // verdicts; the bare Value::truthy short-circuits to `true` for
            // any unrecognised kind, which would let user predicates
            // returning instances silently never terminate the iteration.
            if _interp.truthy_value(&verdict)? {
                Ok(item)
            } else {
                inst.borrow_mut()
                    .attrs
                    .insert("_done", Value::bool_(true));
                Err(PyError::named("StopIteration", String::new()))
            }
        }
    }

    /// CPython: itertools.dropwhile(predicate, iterable) — skip while
    /// `predicate(x)` is truthy; yield everything from the first falsy
    /// element onwards (predicate is *not* re-evaluated after the
    /// flip).
    /// <https://docs.python.org/3/library/itertools.html#itertools.dropwhile>
    class dropwhile {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("dropwhile", user)?;
            if user.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("dropwhile expected 2 arguments, got {}", user.len()),
                ));
            }
            let iter = make_iter(_interp, &user[1].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_pred", user[0].value.clone());
            a.attrs.insert("_iter", iter);
            // `_started` flips True once we've seen the first non-matching
            // element; from then on we just drain `_iter` unconditionally.
            a.attrs.insert("_started", Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let started = matches!(
                inst.borrow().attrs.get("_started").map(|v| v.kind()),
                Some(ValueKind::Bool(true))
            );
            let iter = inst
                .borrow()
                .attrs
                .get("_iter")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            if started {
                return _interp.call_next(&iter, None);
            }
            let pred = inst
                .borrow()
                .attrs
                .get("_pred")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            // Drain while predicate true.
            loop {
                let item = _interp.call_next(&iter, None)?;
                let verdict = _interp.call_function_expanded(
                    pred.clone(),
                    &[ExpandedCallArg {
                        name: None,
                        value: item.clone(),
                    }],
                )?;
                // See takewhile — must dispatch __bool__/__len__ for PyInstance verdicts.
                if !_interp.truthy_value(&verdict)? {
                    inst.borrow_mut()
                        .attrs
                        .insert("_started", Value::bool_(true));
                    return Ok(item);
                }
            }
        }
    }

    /// CPython: itertools.starmap(function, iterable) — like `map`, but
    /// unpacks each element as positional args to `function`.
    /// <https://docs.python.org/3/library/itertools.html#itertools.starmap>
    class starmap {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("starmap", user)?;
            if user.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("starmap expected 2 arguments, got {}", user.len()),
                ));
            }
            let iter = make_iter(_interp, &user[1].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_func", user[0].value.clone());
            a.attrs.insert("_iter", iter);
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (func, iter) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_func").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let pack = _interp.call_next(&iter, None)?;
            // Unpack — any iterable, including generators and instances
            // with `__iter__`/`__next__`.  `collect_iterable` drives the
            // iterator protocol, unlike the bare `iter_values` helper
            // which only handles built-in containers.
            let unpacked: Vec<ExpandedCallArg> = _interp
                .collect_iterable(&pack)?
                .into_iter()
                .map(|v| ExpandedCallArg { name: None, value: v })
                .collect();
            _interp.call_function_expanded(func, &unpacked)
        }
    }

    /// CPython: itertools.accumulate(iterable, func=operator.add, *,
    /// initial=None) — running fold.  Yields the initial accumulator
    /// first when `initial` is set, then the running result after each
    /// step.  `func=None` defaults to addition.
    /// <https://docs.python.org/3/library/itertools.html#itertools.accumulate>
    class accumulate {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // Separate `initial` (keyword-only) from positionals.
            //
            // CPython treats `initial=None` as "no initial" — equivalent
            // to omitting the argument.  Match that by filtering out None
            // here, so the downstream `use_initial` flag stays meaningful.
            let mut positional: Vec<Value> = Vec::new();
            let mut initial: Option<Value> = None;
            for a in &args[1..] {
                match a.name.as_deref() {
                    Some("initial") => {
                        if !a.value.is_none() {
                            initial = Some(a.value.clone());
                        }
                    }
                    Some(other) => return Err(PyError::named(
                        "TypeError",
                        format!("'{other}' is an invalid keyword argument for accumulate()"),
                    )),
                    None => positional.push(a.value.clone()),
                }
            }
            if positional.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "accumulate() missing required argument 'iterable' (pos 1)".to_string(),
                ));
            }
            if positional.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "accumulate() takes at most 2 positional arguments ({} given)",
                        positional.len()
                    ),
                ));
            }
            let iter = make_iter(_interp, &positional[0])?;
            let func = positional.get(1).cloned().unwrap_or_else(Value::none);
            let use_initial = initial.is_some();
            let mut a = inst.borrow_mut();
            a.attrs.insert("_func", func);
            a.attrs.insert("_iter", iter);
            a.attrs.insert("_acc", initial.unwrap_or_else(Value::none));
            a.attrs.insert("_use_initial", Value::bool_(use_initial));
            // `_started`: false until we've yielded something.  The
            // first-yield path branches on `_use_initial`; everything
            // after walks the source-pull-and-fold loop.
            a.attrs.insert("_started", Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let started = matches!(
                inst.borrow().attrs.get("_started").map(|v| v.kind()),
                Some(ValueKind::Bool(true))
            );
            if !started {
                // First yield: either `initial` (no source pull) or the
                // first source element (which becomes the seed acc).
                inst.borrow_mut()
                    .attrs
                    .insert("_started", Value::bool_(true));
                let use_initial = matches!(
                    inst.borrow().attrs.get("_use_initial").map(|v| v.kind()),
                    Some(ValueKind::Bool(true))
                );
                if use_initial {
                    return inst
                        .borrow()
                        .attrs
                        .get("_acc")
                        .cloned()
                        .ok_or_else(|| internal(FN_NAME));
                }
                let iter = inst
                    .borrow()
                    .attrs
                    .get("_iter")
                    .cloned()
                    .ok_or_else(|| internal(FN_NAME))?;
                let first = _interp.call_next(&iter, None)?;
                inst.borrow_mut()
                    .attrs
                    .insert("_acc", first.clone());
                return Ok(first);
            }
            // Steady state: pull next from source, fold with acc.
            let (func, iter, acc) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_func").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_acc").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let nxt = _interp.call_next(&iter, None)?;
            let new_acc = if func.is_none() {
                _interp.eval_binary(acc, crate::ast::BinaryOp::Add, nxt)?
            } else {
                _interp.call_function_expanded(
                    func,
                    &[
                        ExpandedCallArg { name: None, value: acc },
                        ExpandedCallArg { name: None, value: nxt },
                    ],
                )?
            };
            inst.borrow_mut()
                .attrs
                .insert("_acc", new_acc.clone());
            Ok(new_acc)
        }
    }

    /// CPython: itertools.product(*iterables, repeat=1) — Cartesian
    /// product.  Each iterable is materialised eagerly because the
    /// product algorithm walks each pool by index repeatedly.
    /// <https://docs.python.org/3/library/itertools.html#itertools.product>
    class product {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // `repeat` is keyword-only.
            let mut positional: Vec<Value> = Vec::new();
            let mut repeat: i64 = 1;
            for a in &args[1..] {
                match a.name.as_deref() {
                    // `repeat=` honors the `__index__` protocol (#2022).
                    Some("repeat") => {
                        repeat = _interp.value_to_isize(
                            &a.value,
                            "Python int too large to convert to C ssize_t",
                        )?;
                    }
                    Some(other) => return Err(PyError::named(
                        "TypeError",
                        format!("'{other}' is an invalid keyword argument for product()"),
                    )),
                    None => positional.push(a.value.clone()),
                }
            }
            if repeat < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "repeat argument cannot be negative".to_string(),
                ));
            }
            let repeat = usize::try_from(repeat)
                .map_err(|_| PyError::named("MemoryError", String::new()))?;
            // Build the pool list — each input iterable materialised, the
            // whole sequence repeated `repeat` times.  Use
            // `collect_iterable` (not the bare `iter_values`) so generator
            // and `__iter__`/`__next__`-class sources work.
            //
            // Reserve the repeated dimension count before running user code.
            // The old `positional.len() * repeat` / `Vec::with_capacity`
            // pair could overflow or abort on allocation failure.
            let mut pools =
                combinatoric_cursors::reserve_product_pools(positional.len(), repeat)?;
            // Materialise each distinct input once. Repeated dimensions share
            // the private, never-mutated list backing through Value's O(1) Rc
            // clone instead of cloning every element for every repeat.
            let single_pass: Vec<Value> = if repeat == 0 || positional.is_empty() {
                Vec::new()
            } else {
                let mut materialized =
                    combinatoric_cursors::reserve_distinct_pools(positional.len())?;
                for value in &positional {
                    materialized.push(Value::list(_interp.collect_iterable(value)?));
                }
                materialized
            };
            if !single_pass.is_empty() {
                for _ in 0..repeat {
                    pools.extend(single_pass.iter().cloned());
                }
            }
            inst.borrow_mut().attrs.insert(
                "_cursor",
                combinatoric_cursors::product_cursor_value(pools)?,
            );
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            match combinatoric_cursors::next_product(&inst, FN_NAME)? {
                Some(tuple) => Ok(Value::tuple(tuple)),
                None => Err(PyError::named("StopIteration", String::new())),
            }
        }
    }

    /// CPython: itertools.combinations(iterable, r) — r-length tuples
    /// in input order, no repeats.
    /// <https://docs.python.org/3/library/itertools.html#itertools.combinations>
    class combinations {
        iter_self;
        fn __init__(args) -> Result<Value> {
            init_combo_state(_interp, args, "combinations", /* with_replacement = */ false)
        }

        fn __next__(args) -> Result<Value> {
            advance_combinations(args, "combinations", /* with_replacement = */ false)
        }
    }

    /// CPython: itertools.combinations_with_replacement(iterable, r) —
    /// r-length tuples in input order, repeats allowed.
    /// <https://docs.python.org/3/library/itertools.html#itertools.combinations_with_replacement>
    class combinations_with_replacement {
        iter_self;
        fn __init__(args) -> Result<Value> {
            init_combo_state(
                _interp,
                args,
                "combinations_with_replacement",
                /* with_replacement = */ true,
            )
        }

        fn __next__(args) -> Result<Value> {
            advance_combinations(
                args,
                "combinations_with_replacement",
                /* with_replacement = */ true,
            )
        }
    }

    /// CPython: itertools.permutations(iterable, r=None) — r-length
    /// permutations in lexicographic order.  `r=None` defaults to the
    /// pool size.
    /// <https://docs.python.org/3/library/itertools.html#itertools.permutations>
    class permutations {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("permutations", user)?;
            if user.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "permutations() missing required argument 'iterable' (pos 1)".to_string(),
                ));
            }
            if user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("permutations() takes at most 2 arguments ({} given)", user.len()),
                ));
            }
            // `collect_iterable` walks generators / __iter__ classes too.
            let pool: Vec<Value> = _interp.collect_iterable(&user[0].value)?;
            let r = match user.get(1).map(|argument| argument.value.kind()) {
                None | Some(ValueKind::None) => pool.len(),
                // Preserve the original one-tag common path for plain ints.
                Some(ValueKind::Int(r)) if r >= 0 => r as usize,
                Some(ValueKind::Int(_)) => {
                    return Err(PyError::named(
                        "ValueError",
                        "r must be non-negative".to_string(),
                    ));
                }
                Some(ValueKind::Bool(r)) => r as usize,
                Some(ValueKind::BigInt(_) | ValueKind::PyInstance(_)) => {
                    permutations_non_plain_r(_interp, &user[1].value)?
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "Expected int as r".to_string(),
                    ));
                }
            };
            // The cursor is Rust-only state.  Keeping indices/cycles as Python
            // list attributes forced every yield to decode both lists into
            // Vec<usize>, then allocate and encode them again.
            inst.borrow_mut().attrs.insert(
                "_cursor",
                combinatoric_cursors::permutations_cursor_value(pool, r)?,
            );
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            match combinatoric_cursors::next_permutations(&inst, FN_NAME)? {
                Some(tuple) => Ok(Value::tuple(tuple)),
                None => Err(PyError::named("StopIteration", String::new())),
            }
        }
    }

    /// CPython: itertools.groupby(iterable, key=None) — group
    /// *consecutive* equal elements.  Each yield is `(key, group)` where
    /// `group` is a lazy `_grouper` sub-iterator that shares the single
    /// underlying cursor with the parent `groupby` (mirroring CPython's
    /// `itertools._grouper`).  Because the cursor is shared, advancing the
    /// parent (asking for the next `(key, group)`) makes any previously
    /// yielded group stale — it stops yielding.  See the `_grouper` class
    /// below for the staleness mechanism.
    /// <https://docs.python.org/3/library/itertools.html#itertools.groupby>
    class groupby {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // CPython: `groupby(iterable, key=None)` — `key` accepted both
            // positionally and as a keyword.  Anything else is a TypeError.
            let mut positional: Vec<Value> = Vec::new();
            let mut key_kw: Option<Value> = None;
            for a in &args[1..] {
                match a.name.as_deref() {
                    Some("key") => key_kw = Some(a.value.clone()),
                    Some(other) => return Err(PyError::named(
                        "TypeError",
                        format!("'{other}' is an invalid keyword argument for groupby()"),
                    )),
                    None => positional.push(a.value.clone()),
                }
            }
            if positional.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "groupby() missing required argument 'iterable' (pos 1)".to_string(),
                ));
            }
            if positional.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "groupby() takes at most 2 arguments ({} given)",
                        positional.len()
                    ),
                ));
            }
            if positional.len() == 2 && key_kw.is_some() {
                return Err(PyError::named(
                    "TypeError",
                    "groupby() got multiple values for argument 'key'".to_string(),
                ));
            }
            let iter = make_iter(_interp, &positional[0])?;
            let key_fn = key_kw
                .or_else(|| positional.get(1).cloned())
                .unwrap_or_else(Value::none);
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter", iter);
            a.attrs.insert("_keyfn", key_fn);
            // Shared-cursor state, mirroring CPython's `groupbyobject`:
            //   `_currkey`/`_currvalue` — the lookahead element and its key;
            //   `_has_curr` — whether that lookahead is valid;
            //   `_tgtkey`/`_has_tgt` — key of the group currently handed out;
            //   `_id` — monotonic group counter (staleness token);
            //   `_exhausted` — source iterator is drained.
            a.attrs.insert("_currkey", Value::none());
            a.attrs.insert("_currvalue", Value::none());
            a.attrs.insert("_has_curr", Value::bool_(false));
            a.attrs.insert("_tgtkey", Value::none());
            a.attrs.insert("_has_tgt", Value::bool_(false));
            a.attrs.insert("_id", Value::int(0));
            a.attrs.insert("_exhausted", Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let parent = args[0].value.clone();
            let groupby_class = Rc::clone(&inst.borrow().class);
            // Bump the group id: this invalidates any grouper handed out
            // for the previous group (CPython's `gbo->id` token).
            let new_id = {
                let mut a = inst.borrow_mut();
                let cur = match a.attrs.get("_id").map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => n,
                    _ => return Err(internal(FN_NAME)),
                };
                let next = cur + 1;
                a.attrs.insert("_id", Value::int(next));
                next
            };
            // Skip past any unconsumed items of the previous group: keep
            // consuming the shared cursor while the current key still
            // matches the previous target key (or until we pull the very
            // first element).  This mirrors CPython's `groupby_next`:
            //   while (currkey == tgtkey) { fetch-next }
            // The fetch is *lazy* — we only pull a new element when the
            // cursor is empty (`_has_curr == false`), so a side-effecting
            // source iterator is advanced exactly when CPython advances it.
            loop {
                // Make sure the cursor holds an element to inspect.
                if !groupby_ensure_curr(_interp, &inst, FN_NAME)? {
                    // Source exhausted while skipping — no more groups.
                    return Err(PyError::named("StopIteration", String::new()));
                }
                let (_, currkey, has_tgt, tgtkey) = read_groupby_curr(&inst, FN_NAME)?;
                // Stop as soon as the key differs from the previous group's
                // target (or there is no previous group yet — the very first
                // element).  In that case the cursor stays loaded with the
                // first element of the new group.
                if !has_tgt || !keys_equal(_interp, &currkey, &tgtkey)? {
                    break;
                }
                // Same key: this element belongs to the prior group, skip it
                // by marking the cursor consumed so the next iteration pulls
                // a fresh element.
                groupby_clear_curr(&inst);
            }
            // The cursor now sits on the first element of the new group.
            let currkey = {
                let a = inst.borrow();
                a.attrs.get("_currkey").cloned().ok_or_else(|| internal(FN_NAME))?
            };
            {
                let mut a = inst.borrow_mut();
                a.attrs.insert("_tgtkey", currkey.clone());
                a.attrs.insert("_has_tgt", Value::bool_(true));
            }
            // Hand out a lazy grouper bound to this group id + key.
            let mut attrs = InstanceAttrs::new();
            attrs.insert("_parent", parent);
            attrs.insert("_tgtkey", currkey.clone());
            attrs.insert("_id", Value::int(new_id));
            let grouper = make_grouper_instance(&groupby_class, attrs)?;
            Ok(Value::tuple(vec![currkey, grouper]))
        }
    }

    /// CPython: `itertools._grouper` — the lazy sub-iterator yielded by
    /// `groupby`.  It does not own any data; it reads through its parent
    /// `groupby`'s shared cursor.  It yields `_currvalue` while two
    /// conditions hold: the parent's group id still equals the id this
    /// grouper was created with (`_id`), and the parent's current key still
    /// equals this grouper's target key (`_tgtkey`).  As soon as the parent
    /// is advanced to the next group its id changes, so a stale grouper
    /// immediately raises `StopIteration` — matching CPython's documented
    /// "previous group is no longer visible" behaviour.
    class _grouper {
        iter_self;
        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (parent_val, my_tgtkey, my_id) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_parent").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_tgtkey").cloned().ok_or_else(|| internal(FN_NAME))?,
                    match a.attrs.get("_id").map(|v| v.kind()) {
                        Some(ValueKind::Int(n)) => n,
                        _ => return Err(internal(FN_NAME)),
                    },
                )
            };
            let ValueKind::PyInstance(parent) = parent_val.kind() else {
                return Err(internal(FN_NAME));
            };
            // Staleness: the parent advanced to a new group (id moved on).
            // CPython's `_grouper_next` checks `gbo->id != igo->id` first.
            {
                let a = parent.borrow();
                let parent_id = match a.attrs.get("_id").map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => n,
                    _ => return Err(internal(FN_NAME)),
                };
                if parent_id != my_id {
                    return Err(PyError::named("StopIteration", String::new()));
                }
            }
            // Lazily fetch the next element only if the shared cursor was
            // consumed — matching CPython, where `_grouper_next` pulls from
            // the source iterator only when `gbo->currvalue == NULL`.
            if !groupby_ensure_curr(_interp, parent, FN_NAME)? {
                return Err(PyError::named("StopIteration", String::new()));
            }
            let currkey = {
                let a = parent.borrow();
                a.attrs.get("_currkey").cloned().ok_or_else(|| internal(FN_NAME))?
            };
            if !keys_equal(_interp, &currkey, &my_tgtkey)? {
                // Key no longer matches this grouper's target: group is done.
                // Leave the cursor loaded so the parent's next group sees it.
                return Err(PyError::named("StopIteration", String::new()));
            }
            // Consume the current value (clear the cursor so the next call —
            // whether from this grouper or the parent — pulls a fresh one).
            let currvalue = {
                let a = parent.borrow();
                a.attrs.get("_currvalue").cloned().ok_or_else(|| internal(FN_NAME))?
            };
            groupby_clear_curr(parent);
            Ok(currvalue)
        }
    }

    /// CPython: itertools.compress(data, selectors) — yield elements of
    /// `data` where the corresponding element of `selectors` is truthy.
    /// Stops when either `data` or `selectors` is exhausted, whichever
    /// comes first.
    /// <https://docs.python.org/3/library/itertools.html#itertools.compress>
    class compress {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("compress", user)?;
            if user.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "compress() missing required argument 'data' (pos 1)".to_string(),
                ));
            }
            if user.len() == 1 {
                return Err(PyError::named(
                    "TypeError",
                    "compress() missing required argument 'selectors' (pos 2)".to_string(),
                ));
            }
            if user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("compress() takes at most 2 arguments ({} given)", user.len()),
                ));
            }
            let data_iter = make_iter(_interp, &user[0].value)?;
            let selectors_iter = make_iter(_interp, &user[1].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_data", data_iter);
            a.attrs.insert("_selectors", selectors_iter);
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (data_iter, sel_iter) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_data").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_selectors").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            // Consume in lockstep: pull one from each iterator at a time.
            // Stop as soon as either is exhausted (StopIteration propagates
            // naturally from call_next when the selector side runs out; for
            // the data side we forward it directly).
            loop {
                // Pull next data item — exhaustion propagates as StopIteration.
                let item = _interp.call_next(&data_iter, None)?;
                // Pull next selector — exhaustion propagates as StopIteration.
                let selector = _interp.call_next(&sel_iter, None)?;
                // Dispatch __bool__ / __len__ so PyInstance verdicts work.
                if _interp.truthy_value(&selector)? {
                    return Ok(item);
                }
                // Selector was falsy — skip this pair and try the next one.
            }
        }
    }

    /// CPython: itertools.zip_longest(*iterables, fillvalue=None) — like
    /// zip() but continues until the longest iterable is exhausted,
    /// substituting `fillvalue` for shorter ones.
    /// <https://docs.python.org/3/library/itertools.html#itertools.zip_longest>
    class zip_longest {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let mut positional: Vec<Value> = Vec::new();
            let mut fillvalue = Value::none();
            for a in &args[1..] {
                match a.name.as_deref() {
                    Some("fillvalue") => fillvalue = a.value.clone(),
                    Some(_other) => return Err(PyError::named(
                        "TypeError",
                        "zip_longest() got an unexpected keyword argument".to_string(),
                    )),
                    None => positional.push(a.value.clone()),
                }
            }
            // Build lazy iterators from each input through the shared
            // iteration-domain classifier.
            let iters: Vec<Value> = positional
                .iter()
                .map(|v| make_iter(_interp, v))
                .collect::<Result<_>>()?;
            let n = iters.len();
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iters", Value::list(iters));
            a.attrs.insert("_fillvalue", fillvalue);
            // `_active` tracks how many iterables have not yet raised
            // StopIteration.  Once it reaches zero we stop.
            a.attrs.insert("_active", Value::int(n as i64));
            // `_done` is a parallel bool list (one per iterator).
            a.attrs.insert(
                "_done",
                Value::list(vec![Value::bool_(false); n]),
            );
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let active = match inst.borrow().attrs.get("_active").map(|v| v.kind()) {
                Some(ValueKind::Int(n)) => n,
                _ => return Err(internal(FN_NAME)),
            };
            if active == 0 {
                return Err(PyError::named("StopIteration", String::new()));
            }
            let (iters, fillvalue) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_iters").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_fillvalue").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let n = match iters.kind() {
                ValueKind::List(lst) => lst.len(),
                _ => return Err(internal(FN_NAME)),
            };
            let mut tuple: Vec<Value> = Vec::with_capacity(n);
            let mut new_active = active;
            // Read current _done flags.
            let done_val = inst
                .borrow()
                .attrs
                .get("_done")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            // Snapshot the tiny parallel flag vector. In particular, do not
            // retain its List Ref guard while an iterator's user __next__
            // implementation can re-enter this zip_longest instance.
            let mut new_done = match done_val.kind() {
                ValueKind::List(lst) => lst.to_vec(),
                _ => return Err(internal(FN_NAME)),
            };
            if new_done.len() != n {
                return Err(internal(FN_NAME));
            }
            for (i, done) in new_done.iter_mut().enumerate() {
                let already_done = matches!(done.kind(), ValueKind::Bool(true));
                if already_done {
                    tuple.push(fillvalue.clone());
                } else {
                    // Clone only the iterator needed by this step, releasing
                    // the `_iters` List guard before arbitrary user code runs.
                    let iter = match iters.kind() {
                        ValueKind::List(lst) => {
                            lst.get(i).cloned().ok_or_else(|| internal(FN_NAME))?
                        }
                        _ => return Err(internal(FN_NAME)),
                    };
                    match _interp.call_next(&iter, None) {
                        Ok(v) => tuple.push(v),
                        Err(e) if is_stop_iteration(&e) => {
                            tuple.push(fillvalue.clone());
                            *done = Value::bool_(true);
                            new_active -= 1;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            inst.borrow_mut()
                .attrs
                .insert("_active", Value::int(new_active));
            inst.borrow_mut()
                .attrs
                .insert("_done", Value::list(new_done));
            // CPython behaviour: when the last active iterator(s) raise
            // StopIteration in the same step, zip_longest raises
            // StopIteration too — it does NOT yield the all-fill row.
            if new_active == 0 {
                return Err(PyError::named("StopIteration", String::new()));
            }
            Ok(Value::tuple(tuple))
        }
    }

    /// CPython: itertools.filterfalse(predicate, iterable) — yield
    /// elements for which `predicate(elem)` is falsy.  If predicate is
    /// `None`, yield elements that are themselves falsy.
    /// <https://docs.python.org/3/library/itertools.html#itertools.filterfalse>
    class filterfalse {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("filterfalse", user)?;
            if user.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("filterfalse expected 2 arguments, got {}", user.len()),
                ));
            }
            let iter = make_iter(_interp, &user[1].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_pred", user[0].value.clone());
            a.attrs.insert("_iter", iter);
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (pred, iter) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_pred").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            loop {
                let item = _interp.call_next(&iter, None)?;
                let falsy = if pred.is_none() {
                    // None predicate — test the element itself.
                    !_interp.truthy_value(&item)?
                } else {
                    let verdict = _interp.call_function_expanded(
                        pred.clone(),
                        &[ExpandedCallArg { name: None, value: item.clone() }],
                    )?;
                    !_interp.truthy_value(&verdict)?
                };
                if falsy {
                    return Ok(item);
                }
            }
        }
    }

    /// CPython: itertools.tee(iterable, n=2) — return `n` independent
    /// iterators that all yield the same sequence.  Internally each
    /// iterator draws from a shared VecDeque buffer filled lazily from
    /// the common source.
    /// <https://docs.python.org/3/library/itertools.html#itertools.tee>
    fn tee(args) -> Result<Value> {
        // Parse: tee(iterable) or tee(iterable, n)
        // CPython rejects keyword arguments outright for tee().
        let mut positional: Vec<Value> = Vec::new();
        for a in args {
            match a.name.as_deref() {
                Some(_) => {
                    return Err(PyError::named(
                        "TypeError",
                        "itertools.tee() takes no keyword arguments".to_string(),
                    ))
                }
                None => positional.push(a.value.clone()),
            }
        }
        let got = positional.len();
        if got == 0 {
            return Err(PyError::named(
                "TypeError",
                format!("tee expected at least 1 argument, got {got}"),
            ));
        }
        if got > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("tee expected at most 2 arguments, got {got}"),
            ));
        }
        // `n` honors the `__index__` protocol (#2022); a non-int raises the
        // canonical TypeError, an overflowing bigint raises OverflowError, and
        // `n < 0` raises `ValueError: n must be >= 0`.
        let n: usize = match positional.get(1).cloned() {
            None => 2,
            Some(v) => {
                let n = _interp.value_to_isize(&v, "Python int too large to convert to C ssize_t")?;
                if n < 0 {
                    return Err(PyError::named("ValueError", "n must be >= 0".to_string()));
                }
                n as usize
            }
        };
        Ok(Value::tuple(native_iterators::tee_iterators(
            _interp,
            &positional[0],
            n,
        )?))
    }

    /// CPython: itertools.pairwise(iterable) — yield successive
    /// overlapping pairs: `pairwise('ABCD') → (A,B) (B,C) (C,D)`.
    /// Added in Python 3.10.
    /// <https://docs.python.org/3/library/itertools.html#itertools.pairwise>
    class pairwise {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("pairwise", user)?;
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "pairwise expected 1 argument, got {}",
                        user.len()
                    ),
                ));
            }
            let iter = make_iter(_interp, &user[0].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter", iter);
            a.attrs.insert("_prev", Value::none());
            a.attrs.insert("_started", Value::bool_(false));
            a.attrs.insert("_exhausted", Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            check_not_exhausted(&inst, "_exhausted")?;
            let (mut prev, iter, started) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_prev").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                    matches!(
                        a.attrs.get("_started").map(|value| value.kind()),
                        Some(ValueKind::Bool(true))
                    ),
                )
            };
            if !started {
                prev = match _interp.call_next(&iter, None) {
                    Ok(value) => value,
                    Err(e) if is_stop_iteration(&e) => {
                        inst.borrow_mut()
                            .attrs
                            .insert("_exhausted", Value::bool_(true));
                        return Err(e);
                    }
                    Err(e) => return Err(e),
                };
                let mut attrs = inst.borrow_mut();
                attrs.attrs.insert("_prev", prev.clone());
                attrs.attrs.insert("_started", Value::bool_(true));
            }
            match _interp.call_next(&iter, None) {
                Ok(next) => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_prev", next.clone());
                    Ok(Value::tuple(vec![prev, next]))
                }
                Err(e) if is_stop_iteration(&e) => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_exhausted", Value::bool_(true));
                    Err(PyError::named("StopIteration", String::new()))
                }
                Err(e) => Err(e),
            }
        }
    }

    /// CPython: itertools.batched(iterable, n) — yield tuples of length
    /// `n` from `iterable`; the last tuple may be shorter.  `n` must be
    /// >= 1.  Added in Python 3.12.
    /// <https://docs.python.org/3/library/itertools.html#itertools.batched>
    class batched {
        iter_self;
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded("batched", user)?;
            if user.len() < 2 {
                return Err(PyError::named(
                    "TypeError",
                    "batched() missing required argument 'n' (pos 2)".to_string(),
                ));
            }
            if user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("batched() takes at most 2 arguments ({} given)", user.len()),
                ));
            }
            // `n` honors the `__index__` protocol (#2022); a non-int raises the
            // canonical TypeError, an overflowing bigint raises OverflowError,
            // and `n < 1` raises `ValueError: n must be at least one`.
            let n = _interp.value_to_isize(
                &user[1].value,
                "Python int too large to convert to C ssize_t",
            )?;
            if n < 1 {
                return Err(PyError::named("ValueError", "n must be at least one".to_string()));
            }
            let iter = make_iter(_interp, &user[0].value)?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter", iter);
            a.attrs.insert("_n", Value::int(n));
            a.attrs.insert("_exhausted", Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            check_not_exhausted(&inst, "_exhausted")?;
            let (iter, n) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                    match a.attrs.get("_n").map(|v| v.kind()) {
                        Some(ValueKind::Int(n)) => n as usize,
                        _ => return Err(internal(FN_NAME)),
                    },
                )
            };
            let mut batch: Vec<Value> = Vec::with_capacity(n);
            for _ in 0..n {
                match _interp.call_next(&iter, None) {
                    Ok(v) => batch.push(v),
                    Err(e) if is_stop_iteration(&e) => {
                        inst.borrow_mut()
                            .attrs
                            .insert("_exhausted", Value::bool_(true));
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if batch.is_empty() {
                Err(PyError::named("StopIteration", String::new()))
            } else {
                Ok(Value::tuple(batch))
            }
        }
    }
}

// Non-macro responsibilities are kept in focused implementation fragments.
include!("itertools/support.inc.rs");
include!("itertools/islice_helpers.inc.rs");
include!("itertools/groupby_helpers.inc.rs");
include!("itertools/combinatoric_adapters.inc.rs");
include!("itertools/class_registry.inc.rs");
