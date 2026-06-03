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
// `chain` is the only function that's still lazy *across sources* but
// eager *within* each source (it pre-dates the class-based pattern;
// converting it is tracked separately).
//
// Reference: <https://docs.python.org/3/library/itertools.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::value::{PyInstance, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;
use std::cell::RefCell;
use std::rc::Rc;

pyrust_module! {
    /// CPython: itertools.chain(*iterables) — concatenate iterables.
    /// Lazy across sources (each is materialised only when reached).
    /// <https://docs.python.org/3/library/itertools.html#itertools.chain>
    fn chain(args) -> Result<Value> {
        reject_keyword_args_expanded("chain", args)?;
        // Pre-materialise user `PyInstance` AND `Generator` sources so
        // user `__iter__` dispatch / generator resumption (both of
        // which need the interpreter) happen here instead of inside
        // the registry callback, which can only walk
        // `NativeIterFrame` generators (#446).  Re-uses the same
        // helper `enumerate`/`zip`/`reversed` call so behaviour stays
        // consistent across the lazy iter family.
        let sources: Vec<Value> = args
            .iter()
            .map(|a| super::builtins::materialize_user_iter(_interp, a.value.clone()))
            .collect::<Result<_>>()?;
        Ok(pyrust_builtins::iter_helpers::chain(sources))
    }

    /// CPython: itertools.chain.from_iterable(iterable) — the alternate
    /// constructor that takes a *single* iterable whose elements are the
    /// iterables to chain.  Equivalent to `chain(*iterable)` but lazy: the
    /// outer iterable is consumed one element at a time, and each inner
    /// iterable is only iterated when reached, so an infinite outer source
    /// works up to the consumed point.  Exposed via attribute access on the
    /// `chain` builtin (see `env.rs::get_attr` BuiltinFunction arm).
    /// <https://docs.python.org/3/library/itertools.html#itertools.chain.from_iterable>
    fn chain_from_iterable(args) -> Result<Value> {
        reject_keyword_args_expanded("chain.from_iterable", args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "chain.from_iterable() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        }
        // `iter(arg)` over the outer iterable.  This does not consume any
        // element yet (for a generator source it just returns the generator);
        // the first element is pulled on the first `__next__`, matching
        // CPython's lazy timing.
        let outer = make_iter(_interp, &args[0].value)?;
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert("_outer".to_string(), outer);
        attrs.insert("_inner".to_string(), Value::none());
        make_itertools_instance("_chain_from_iterable", attrs)
    }

    /// CPython: the iterator returned by `itertools.chain.from_iterable`.
    /// Holds the outer iterator (`_outer`) and the current inner iterator
    /// (`_inner`, `None` until the first inner iterable is reached).  Inner
    /// iterables are pulled from the outer source on demand and `iter()`-ed
    /// lazily, so a non-iterable element raises `TypeError` only when
    /// reached.
    class _chain_from_iterable {
        iter_self;
        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            loop {
                let inner = inst
                    .borrow()
                    .attrs
                    .get("_inner")
                    .cloned()
                    .ok_or_else(|| internal(FN_NAME))?;
                if inner.is_none() {
                    // No current inner iterator — pull the next inner iterable
                    // from the outer source (StopIteration propagates to end
                    // the whole chain), then `iter()` it lazily.  A
                    // non-iterable element raises TypeError here, matching
                    // CPython's "'<type>' object is not iterable".
                    let outer = inst
                        .borrow()
                        .attrs
                        .get("_outer")
                        .cloned()
                        .ok_or_else(|| internal(FN_NAME))?;
                    let next_iterable = _interp.call_next(&outer, None)?;
                    let new_inner = make_iter(_interp, &next_iterable)?;
                    inst.borrow_mut()
                        .attrs
                        .insert("_inner".to_string(), new_inner);
                    continue;
                }
                // Drain the current inner iterator; on exhaustion drop it and
                // loop back to fetch the next inner iterable.
                match _interp.call_next(&inner, None) {
                    Ok(v) => return Ok(v),
                    Err(e) if is_stop_iteration(&e) => {
                        inst.borrow_mut()
                            .attrs
                            .insert("_inner".to_string(), Value::none());
                    }
                    Err(e) => return Err(e),
                }
            }
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
            for _ in 0..start {
                match _interp.call_next(&iter, None) {
                    Ok(_) => {}
                    Err(e) if is_stop_iteration(&e) => break,
                    Err(e) => return Err(e),
                }
            }
            let remaining: Option<i64> = stop.map(|s| (s - start).max(0));
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert(
                "_remaining_stop".to_string(),
                remaining.map_or_else(Value::none, Value::int),
            );
            a.attrs.insert("_step".to_string(), Value::int(step));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (iter, remaining, step) = read_islice_state(&inst, FN_NAME)?;
            if let Some(r) = remaining
                && r <= 0
            {
                return Err(PyError::named("StopIteration", String::new()));
            }
            let item = _interp.call_next(&iter, None)?;
            let mut tail_exhausted = false;
            for _ in 0..(step - 1) {
                match _interp.call_next(&iter, None) {
                    Ok(_) => {}
                    Err(e) if is_stop_iteration(&e) => {
                        tail_exhausted = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            let new_remaining = remaining.map(|r| if tail_exhausted { 0 } else { r - step });
            inst.borrow_mut().attrs.insert(
                "_remaining_stop".to_string(),
                new_remaining.map_or_else(Value::none, Value::int),
            );
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
            a.attrs.insert("_cur".to_string(), start);
            a.attrs.insert("_step".to_string(), step);
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
            inst.borrow_mut().attrs.insert("_cur".to_string(), next);
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
                Ok(Value::string(format!("count({})", cur.repr())))
            } else {
                Ok(Value::string(format!("count({}, {})", cur.repr(), step.repr())))
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
            a.attrs.insert("_object".to_string(), object);
            a.attrs.insert(
                "_remaining".to_string(),
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
                        .insert("_remaining".to_string(), Value::int(n - 1));
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
            let obj_repr =
                crate::builtin_modules::builtins::render_value_repr(_interp, &object)?;
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
            a.attrs.insert("_iter".to_string(), iter);
            // `_cache` accumulates each yielded element during the first
            // pass; once `_iter` exhausts, we walk `_cache` indefinitely.
            a.attrs.insert("_cache".to_string(), Value::list(Vec::new()));
            a.attrs.insert("_pos".to_string(), Value::int(0));
            a.attrs.insert("_first_pass".to_string(), Value::bool_(true));
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
                            .insert("_first_pass".to_string(), Value::bool_(false));
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
                .insert("_pos".to_string(), Value::int((pos + 1) % cache_len as i64));
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
            a.attrs.insert("_pred".to_string(), user[0].value.clone());
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert("_done".to_string(), Value::bool_(false));
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
                    .insert("_done".to_string(), Value::bool_(true));
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
            a.attrs.insert("_pred".to_string(), user[0].value.clone());
            a.attrs.insert("_iter".to_string(), iter);
            // `_started` flips True once we've seen the first non-matching
            // element; from then on we just drain `_iter` unconditionally.
            a.attrs.insert("_started".to_string(), Value::bool_(false));
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
                        .insert("_started".to_string(), Value::bool_(true));
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
            a.attrs.insert("_func".to_string(), user[0].value.clone());
            a.attrs.insert("_iter".to_string(), iter);
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
            a.attrs.insert("_func".to_string(), func);
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert("_acc".to_string(), initial.unwrap_or_else(Value::none));
            a.attrs.insert("_use_initial".to_string(), Value::bool_(use_initial));
            // `_started`: false until we've yielded something.  The
            // first-yield path branches on `_use_initial`; everything
            // after walks the source-pull-and-fold loop.
            a.attrs.insert("_started".to_string(), Value::bool_(false));
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
                    .insert("_started".to_string(), Value::bool_(true));
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
                    .insert("_acc".to_string(), first.clone());
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
                .insert("_acc".to_string(), new_acc.clone());
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
            // Build the pool list — each input iterable materialised, the
            // whole sequence repeated `repeat` times.  Use
            // `collect_iterable` (not the bare `iter_values`) so generator
            // and `__iter__`/`__next__`-class sources work.
            let mut pools: Vec<Vec<Value>> =
                Vec::with_capacity(positional.len() * repeat as usize);
            let single_pass: Vec<Vec<Value>> = positional
                .iter()
                .map(|v| _interp.collect_iterable(v))
                .collect::<Result<_>>()?;
            for _ in 0..repeat {
                pools.extend(single_pass.iter().cloned());
            }
            // Three boundary cases:
            //   - `product()` with no iterables → `pools` is empty, the
            //     odometer is zero-width, and we yield one empty tuple.
            //   - `product(*its, repeat=0)` → also yields one empty tuple
            //     (the zero-fold product is the empty product).
            //   - any input iterable is empty AND `repeat > 0` → yield
            //     nothing (an empty pool short-circuits the Cartesian
            //     product to ∅).  `empty_input` flips `_exhausted` to
            //     pre-empt the first `__next__`.
            let empty_input = pools.iter().any(|p| p.is_empty());
            let mut a = inst.borrow_mut();
            a.attrs.insert("_pools".to_string(), Value::list(
                pools.into_iter().map(Value::list).collect(),
            ));
            a.attrs.insert(
                "_indices".to_string(),
                Value::list(Vec::new()),
            );
            a.attrs.insert("_started".to_string(), Value::bool_(false));
            a.attrs.insert("_exhausted".to_string(), Value::bool_(empty_input));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // One immutable borrow does all the reading: pool lengths,
            // current indices, and the element at each index for the
            // outgoing tuple.  No full-pool clone (the old
            // `read_list_of_lists` path was O(total input size) per
            // yield).
            check_not_exhausted(&inst, "_exhausted")?;
            let outcome: ProductStep = {
                let attrs = inst.borrow();
                let started = matches!(
                    attrs.attrs.get("_started").map(|v| v.kind()),
                    Some(ValueKind::Bool(true))
                );
                let pools_outer = match attrs.attrs.get("_pools").map(|v| v.kind()) {
                    Some(ValueKind::List(outer)) => outer,
                    _ => return Err(internal(FN_NAME)),
                };
                let mut indices: Vec<usize> = if started {
                    match attrs.attrs.get("_indices").map(|v| v.kind()) {
                        Some(ValueKind::List(items)) => items
                            .iter()
                            .map(|v| match v.kind() {
                                ValueKind::Int(n) => n as usize,
                                _ => 0,
                            })
                            .collect(),
                        _ => return Err(internal(FN_NAME)),
                    }
                } else {
                    vec![0; pools_outer.len()]
                };
                if started {
                    let mut i = pools_outer.len();
                    let exhausted = loop {
                        if i == 0 {
                            break true;
                        }
                        i -= 1;
                        let len_i = match pools_outer[i].kind() {
                            ValueKind::List(items) => items.len(),
                            _ => 0,
                        };
                        indices[i] += 1;
                        if indices[i] < len_i {
                            break false;
                        }
                        indices[i] = 0;
                    };
                    if exhausted {
                        ProductStep::Exhausted
                    } else {
                        ProductStep::Yield {
                            tuple: tuple_from_pools(&pools_outer[..], &indices),
                            indices,
                            already_started: true,
                        }
                    }
                } else {
                    ProductStep::Yield {
                        tuple: tuple_from_pools(&pools_outer[..], &indices),
                        indices,
                        already_started: false,
                    }
                }
            };
            match outcome {
                ProductStep::Exhausted => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_exhausted".to_string(), Value::bool_(true));
                    Err(PyError::named("StopIteration", String::new()))
                }
                ProductStep::Yield {
                    tuple,
                    indices,
                    already_started,
                } => {
                    if !already_started {
                        inst.borrow_mut()
                            .attrs
                            .insert("_started".to_string(), Value::bool_(true));
                    }
                    write_indices(&inst, "_indices", &indices);
                    Ok(Value::tuple(tuple))
                }
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
            // CPython splits negative-r (ValueError) from non-int-r
            // (TypeError); match that so user `except` blocks behave
            // identically.
            let r = match user.get(1).map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => pool.len(),
                Some(ValueKind::Int(n)) if n >= 0 => n as usize,
                Some(ValueKind::Int(_)) => return Err(PyError::named(
                    "ValueError",
                    "r must be non-negative".to_string(),
                )),
                Some(ValueKind::Bool(b)) => b as usize,
                _ => return Err(PyError::named(
                    "TypeError",
                    "Expected int as r".to_string(),
                )),
            };
            // CPython's algorithm: keep `indices` (running combination) and
            // `cycles` (countdowns per position).  If r > pool, the
            // generator is immediately exhausted — short-circuit the
            // cycles computation (where `pool.len() - i` would underflow
            // for `r > pool.len() + 1`).
            let exhausted = r > pool.len();
            let indices: Vec<usize> = (0..pool.len()).collect();
            let cycles: Vec<usize> = if exhausted {
                Vec::new()
            } else {
                (0..r).map(|i| pool.len() - i).collect()
            };
            let mut a = inst.borrow_mut();
            a.attrs.insert("_pool".to_string(), Value::list(pool));
            a.attrs.insert("_r".to_string(), Value::int(r as i64));
            a.attrs.insert(
                "_indices".to_string(),
                Value::list(indices.into_iter().map(|i| Value::int(i as i64)).collect()),
            );
            a.attrs.insert(
                "_cycles".to_string(),
                Value::list(cycles.into_iter().map(|i| Value::int(i as i64)).collect()),
            );
            a.attrs.insert("_started".to_string(), Value::bool_(false));
            a.attrs.insert("_exhausted".to_string(), Value::bool_(exhausted));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // Single immutable borrow to read pool, r, indices, cycles,
            // started — and build the outgoing tuple by direct index
            // lookup.  The old version did `pool.to_vec()` per yield,
            // making each step O(n).
            enum Step {
                Yield {
                    tuple: Vec<Value>,
                    indices: Vec<usize>,
                    cycles: Vec<usize>,
                    already_started: bool,
                },
                Exhausted,
            }
            check_not_exhausted(&inst, "_exhausted")?;
            let outcome: Step = {
                let attrs = inst.borrow();
                let pool_items = match attrs.attrs.get("_pool").map(|v| v.kind()) {
                    Some(ValueKind::List(items)) => items,
                    _ => return Err(internal(FN_NAME)),
                };
                let r = match attrs.attrs.get("_r").map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => n as usize,
                    _ => return Err(internal(FN_NAME)),
                };
                let mut indices: Vec<usize> = read_indices(&inst, "_indices", FN_NAME)?;
                let mut cycles: Vec<usize> = read_indices(&inst, "_cycles", FN_NAME)?;
                let started = matches!(
                    attrs.attrs.get("_started").map(|v| v.kind()),
                    Some(ValueKind::Bool(true))
                );
                if started {
                    let n = pool_items.len();
                    let mut i = r;
                    let exhausted = loop {
                        if i == 0 {
                            break true;
                        }
                        i -= 1;
                        cycles[i] -= 1;
                        if cycles[i] == 0 {
                            let head = indices[i];
                            for k in i..(n - 1) {
                                indices[k] = indices[k + 1];
                            }
                            indices[n - 1] = head;
                            cycles[i] = n - i;
                        } else {
                            let j = n - cycles[i];
                            indices.swap(i, j);
                            break false;
                        }
                    };
                    if exhausted {
                        Step::Exhausted
                    } else {
                        Step::Yield {
                            tuple: tuple_from_pool(&pool_items[..], &indices, r),
                            indices,
                            cycles,
                            already_started: true,
                        }
                    }
                } else {
                    Step::Yield {
                        tuple: tuple_from_pool(&pool_items[..], &indices, r),
                        indices,
                        cycles,
                        already_started: false,
                    }
                }
            };
            let (tuple, indices, cycles, already_started) = match outcome {
                Step::Exhausted => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_exhausted".to_string(), Value::bool_(true));
                    return Err(PyError::named("StopIteration", String::new()));
                }
                Step::Yield {
                    tuple,
                    indices,
                    cycles,
                    already_started,
                } => (tuple, indices, cycles, already_started),
            };
            if !already_started {
                inst.borrow_mut()
                    .attrs
                    .insert("_started".to_string(), Value::bool_(true));
            }
            write_indices(&inst, "_indices", &indices);
            write_indices(&inst, "_cycles", &cycles);
            Ok(Value::tuple(tuple))
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
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert("_keyfn".to_string(), key_fn);
            // Shared-cursor state, mirroring CPython's `groupbyobject`:
            //   `_currkey`/`_currvalue` — the lookahead element and its key;
            //   `_has_curr` — whether that lookahead is valid;
            //   `_tgtkey`/`_has_tgt` — key of the group currently handed out;
            //   `_id` — monotonic group counter (staleness token);
            //   `_exhausted` — source iterator is drained.
            a.attrs.insert("_currkey".to_string(), Value::none());
            a.attrs.insert("_currvalue".to_string(), Value::none());
            a.attrs.insert("_has_curr".to_string(), Value::bool_(false));
            a.attrs.insert("_tgtkey".to_string(), Value::none());
            a.attrs.insert("_has_tgt".to_string(), Value::bool_(false));
            a.attrs.insert("_id".to_string(), Value::int(0));
            a.attrs.insert("_exhausted".to_string(), Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let parent = args[0].value.clone();
            // Bump the group id: this invalidates any grouper handed out
            // for the previous group (CPython's `gbo->id` token).
            let new_id = {
                let mut a = inst.borrow_mut();
                let cur = match a.attrs.get("_id").map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => n,
                    _ => return Err(internal(FN_NAME)),
                };
                let next = cur + 1;
                a.attrs.insert("_id".to_string(), Value::int(next));
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
                a.attrs.insert("_tgtkey".to_string(), currkey.clone());
                a.attrs.insert("_has_tgt".to_string(), Value::bool_(true));
            }
            // Hand out a lazy grouper bound to this group id + key.
            let mut attrs: IndexMap<String, Value> = IndexMap::new();
            attrs.insert("_parent".to_string(), parent);
            attrs.insert("_tgtkey".to_string(), currkey.clone());
            attrs.insert("_id".to_string(), Value::int(new_id));
            let grouper = make_itertools_instance("_grouper", attrs)?;
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
            if !groupby_ensure_curr(_interp, &parent, FN_NAME)? {
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
            groupby_clear_curr(&parent);
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
            a.attrs.insert("_data".to_string(), data_iter);
            a.attrs.insert("_selectors".to_string(), selectors_iter);
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
            // Build iterators from each input; pre-materialise only
            // PyInstance / Generator sources (same as chain/compress).
            let iters: Vec<Value> = positional
                .iter()
                .map(|v| make_iter(_interp, v))
                .collect::<Result<_>>()?;
            let n = iters.len();
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iters".to_string(), Value::list(iters));
            a.attrs.insert("_fillvalue".to_string(), fillvalue);
            // `_active` tracks how many iterables have not yet raised
            // StopIteration.  Once it reaches zero we stop.
            a.attrs.insert("_active".to_string(), Value::int(n as i64));
            // `_done` is a parallel bool list (one per iterator).
            a.attrs.insert(
                "_done".to_string(),
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
            let iter_list = match iters.kind() {
                ValueKind::List(lst) => lst,
                _ => return Err(internal(FN_NAME)),
            };
            let n = iter_list.len();
            let mut tuple: Vec<Value> = Vec::with_capacity(n);
            let mut new_active = active;
            // Read current _done flags.
            let done_val = inst
                .borrow()
                .attrs
                .get("_done")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            let done_vals = match done_val.kind() {
                ValueKind::List(lst) => lst,
                _ => return Err(internal(FN_NAME)),
            };
            let mut new_done: Vec<Value> = done_vals.clone();
            for i in 0..n {
                let already_done = matches!(done_vals[i].kind(), ValueKind::Bool(true));
                if already_done {
                    tuple.push(fillvalue.clone());
                } else {
                    match _interp.call_next(&iter_list[i], None) {
                        Ok(v) => tuple.push(v),
                        Err(e) if is_stop_iteration(&e) => {
                            tuple.push(fillvalue.clone());
                            new_done[i] = Value::bool_(true);
                            new_active -= 1;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            inst.borrow_mut()
                .attrs
                .insert("_active".to_string(), Value::int(new_active));
            inst.borrow_mut()
                .attrs
                .insert("_done".to_string(), Value::list(new_done));
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
            a.attrs.insert("_pred".to_string(), user[0].value.clone());
            a.attrs.insert("_iter".to_string(), iter);
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
        // Materialise the source into a list eagerly, then return n list_iterator
        // views all starting at index 0.  Fully lazy tee requires a shared
        // buffer cell (Rc<RefCell<…>>) which can't be stored in Value attrs
        // without an Rc<dyn Any> escape hatch.  Materialising is simpler and
        // matches the observable contract: any use of tee iterators
        // after exhausting the source is safe.
        let items = _interp.collect_iterable(&positional[0])?;
        let shared = Value::list(items);
        // Each tee iterator is a list_iterator-style instance with a
        // `_source` (the shared list) and `_pos` index.
        let mut result: Vec<Value> = Vec::with_capacity(n);
        for _ in 0..n {
            let iter = _interp.call_function_expanded(
                Value::builtin_function("iter"),
                &[ExpandedCallArg { name: None, value: shared.clone() }],
            )?;
            result.push(iter);
        }
        Ok(Value::tuple(result))
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
            // Pull the first element so we always have a `_prev` to pair
            // with.  If the source is empty or has only one element the
            // iterator is immediately exhausted.
            let prev = match _interp.call_next(&iter, None) {
                Ok(v) => v,
                Err(e) if is_stop_iteration(&e) => {
                    // Empty source — mark exhausted immediately.
                    let mut a = inst.borrow_mut();
                    a.attrs.insert("_iter".to_string(), iter);
                    a.attrs.insert("_prev".to_string(), Value::none());
                    a.attrs.insert("_exhausted".to_string(), Value::bool_(true));
                    return Ok(Value::none());
                }
                Err(e) => return Err(e),
            };
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert("_prev".to_string(), prev);
            a.attrs.insert("_exhausted".to_string(), Value::bool_(false));
            Ok(Value::none())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            check_not_exhausted(&inst, "_exhausted")?;
            let (prev, iter) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_prev").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            match _interp.call_next(&iter, None) {
                Ok(next) => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_prev".to_string(), next.clone());
                    Ok(Value::tuple(vec![prev, next]))
                }
                Err(e) if is_stop_iteration(&e) => {
                    inst.borrow_mut()
                        .attrs
                        .insert("_exhausted".to_string(), Value::bool_(true));
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
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert("_n".to_string(), Value::int(n));
            a.attrs.insert("_exhausted".to_string(), Value::bool_(false));
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
                            .insert("_exhausted".to_string(), Value::bool_(true));
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

// ── shared helpers ───────────────────────────────────────────────────────────

/// Method-shared `self` extractor — `args[0]` is the instance.
fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<std::cell::RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(&rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Single source of truth for the "iterator already exhausted →
/// `StopIteration`" prologue shared by every itertools `__next__`.  Reads
/// the boolean sentinel stored under `flag` (`_exhausted` / `_done`) and
/// raises `StopIteration` (empty message, matching CPython) when it's
/// `True`.  Takes the instance and borrows internally so call sites that
/// already hold an `attrs` borrow can call it *before* opening their own
/// borrow block.
fn check_not_exhausted(inst: &Rc<std::cell::RefCell<PyInstance>>, flag: &str) -> Result<()> {
    if matches!(
        inst.borrow().attrs.get(flag).map(|v| v.kind()),
        Some(ValueKind::Bool(true))
    ) {
        return Err(PyError::named("StopIteration", String::new()));
    }
    Ok(())
}

/// Read the (`_iter`, `_remaining_stop`, `_step`) triple for `islice`.
fn read_islice_state(
    inst: &Rc<std::cell::RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(Value, Option<i64>, i64)> {
    let attrs = inst.borrow();
    let iter = attrs
        .attrs
        .get("_iter")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let remaining = match attrs.attrs.get("_remaining_stop").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => Some(n),
        Some(ValueKind::None) | None => None,
        _ => return Err(internal(fn_name)),
    };
    let step = match attrs.attrs.get("_step").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        _ => return Err(internal(fn_name)),
    };
    Ok((iter, remaining, step))
}

/// Extract a non-negative `i64` (or `None`) from an `islice` slot.
fn slice_arg(interp: &mut crate::Interpreter, _fn_name: &str, v: &Value, slot: &str) -> Result<Option<i64>> {
    // CPython's `evaluate_slice_index` honors the `__index__` protocol (an
    // int-subclass or `__index__` object is accepted), but unlike the
    // canonical index contexts it raises a *ValueError* — not a TypeError —
    // for anything that isn't an integer (#2022).  Resolve `__index__` via the
    // shared protocol; only the non-index case takes the ValueError path.
    let resolved = match v.kind() {
        ValueKind::None => return Ok(None),
        ValueKind::Int(n) => return Ok(Some(n)),
        ValueKind::Bool(b) => return Ok(Some(b as i64)),
        ValueKind::PyInstance(_) => {
            interp.value_to_index(v, |_| PyError::named("__pyrust_NotIndex__", String::new()))
        }
        _ => Err(PyError::named("__pyrust_NotIndex__", String::new())),
    };
    let slice_value_err = || {
        let msg = match slot {
            "step" => "Step for islice() must be a positive integer or None.".to_string(),
            "stop" => {
                "Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize."
                    .to_string()
            }
            _ => "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.".to_string(),
        };
        PyError::named("ValueError", msg)
    };
    match resolved {
        Ok(r) => match r.kind() {
            ValueKind::Int(n) => Ok(Some(n)),
            ValueKind::Bool(b) => Ok(Some(b as i64)),
            // A bigint slice bound is out of the `0 <= x <= sys.maxsize` range.
            ValueKind::BigInt(_) => Err(slice_value_err()),
            _ => unreachable!("value_to_index guarantees an integer"),
        },
        Err(PyError::Named(name, _)) if name == "__pyrust_NotIndex__" => Err(slice_value_err()),
        Err(e) => Err(e),
    }
}

/// `count.__init__` validation — start/step must be numeric.
/// BigInt is accepted so `count(10**30)` works the same as
/// `count(10)` (matching Python's arbitrary-precision ints); the
/// running `_cur` may then transition between `Int` and `BigInt`
/// as values cross the i64 boundary, but `eval_binary(Add)`
/// handles both directions of that conversion.
fn require_numeric(v: &Value, _fn_name: &str, _slot: &str) -> Result<()> {
    if matches!(
        v.kind(),
        ValueKind::Int(_) | ValueKind::Float(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
    ) {
        Ok(())
    } else {
        Err(PyError::named(
            "TypeError",
            "a number is required".to_string(),
        ))
    }
}

/// Construct an iterator from an iterable.  Equivalent to Python's
/// `iter(obj)`; we go through the interpreter's `iter` builtin so
/// `__iter__`-providing classes are handled uniformly.
fn make_iter(interp: &mut crate::Interpreter, iterable: &Value) -> Result<Value> {
    interp.call_function_expanded(
        Value::builtin_function("iter"),
        &[ExpandedCallArg {
            name: None,
            value: iterable.clone(),
        }],
    )
}

/// Recognise StopIteration in all PyError forms, including user-defined
/// subclasses carried as `PyError::Raised`.  `class_name_is` walks the base
/// chain for `Raised` variants so this is subclass-aware.
fn is_stop_iteration(e: &PyError) -> bool {
    e.class_name_is("StopIteration")
}

/// Apply `key_fn(item)` if it's callable (i.e. not None); otherwise the
/// item is its own key.
fn compute_key(interp: &mut crate::Interpreter, key_fn: &Value, item: &Value) -> Result<Value> {
    if key_fn.is_none() {
        Ok(item.clone())
    } else {
        interp.call_function_expanded(
            key_fn.clone(),
            &[ExpandedCallArg {
                name: None,
                value: item.clone(),
            }],
        )
    }
}

/// Internal-error shorthand — should never fire in practice; reaching
/// it means an attr was wiped externally or never written by `__init__`.
fn internal(fn_name: &str) -> PyError {
    PyError::Runtime(format!("internal: {fn_name}() instance state corrupted"))
}

/// Compare two group keys for `groupby`/`_grouper` equality.  Goes through
/// `eval_binary(Eq)` + `truthy_value` rather than Rust `==`: Rust-side `==`
/// is identity-based for `PyInstance`, but `groupby(items, key=K)` keys may
/// be user objects whose `__eq__` defines grouping (and may itself return a
/// `PyInstance` routed through `__bool__`/`__len__`).
fn keys_equal(interp: &mut crate::Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let eq_val = interp.eval_binary(a.clone(), crate::ast::BinaryOp::Eq, b.clone())?;
    interp.truthy_value(&eq_val)
}

/// Read the `groupby` shared-cursor lookahead: `(has_curr, currkey,
/// has_tgt, tgtkey)`.
fn read_groupby_curr(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(bool, Value, bool, Value)> {
    let a = inst.borrow();
    let has_curr = matches!(
        a.attrs.get("_has_curr").map(|v| v.kind()),
        Some(ValueKind::Bool(true))
    );
    let has_tgt = matches!(
        a.attrs.get("_has_tgt").map(|v| v.kind()),
        Some(ValueKind::Bool(true))
    );
    let currkey = a
        .attrs
        .get("_currkey")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let tgtkey = a
        .attrs
        .get("_tgtkey")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    Ok((has_curr, currkey, has_tgt, tgtkey))
}

/// Ensure a `groupby`'s shared cursor holds an element.  If the cursor is
/// already loaded (`_has_curr == true`) this is a no-op.  Otherwise it
/// pulls the next item from the source iterator, computes its key, and
/// stores both as the new `_currvalue`/`_currkey` lookahead.  Returns
/// `Ok(true)` if the cursor now holds an element, `Ok(false)` if the source
/// iterator is exhausted.
///
/// The fetch is lazy on purpose: CPython only advances the underlying
/// iterator when its `currvalue` slot is empty, so a side-effecting source
/// is consumed exactly when CPython consumes it.
fn groupby_ensure_curr(
    interp: &mut crate::Interpreter,
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<bool> {
    let (has_curr, iter, key_fn) = {
        let a = inst.borrow();
        let has_curr = matches!(
            a.attrs.get("_has_curr").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        (
            has_curr,
            a.attrs.get("_iter").cloned().ok_or_else(|| internal(fn_name))?,
            a.attrs.get("_keyfn").cloned().ok_or_else(|| internal(fn_name))?,
        )
    };
    if has_curr {
        return Ok(true);
    }
    let item = match interp.call_next(&iter, None) {
        Ok(v) => v,
        Err(e) if is_stop_iteration(&e) => {
            let mut a = inst.borrow_mut();
            a.attrs.insert("_has_curr".to_string(), Value::bool_(false));
            a.attrs.insert("_exhausted".to_string(), Value::bool_(true));
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    let key = compute_key(interp, &key_fn, &item)?;
    let mut a = inst.borrow_mut();
    a.attrs.insert("_currvalue".to_string(), item);
    a.attrs.insert("_currkey".to_string(), key);
    a.attrs.insert("_has_curr".to_string(), Value::bool_(true));
    Ok(true)
}

/// Mark a `groupby`'s shared cursor as consumed, so the next
/// `groupby_ensure_curr` pulls a fresh element.  Mirrors CPython clearing
/// `gbo->currvalue`/`gbo->currkey` after a value is handed out.
fn groupby_clear_curr(inst: &Rc<RefCell<PyInstance>>) {
    let mut a = inst.borrow_mut();
    a.attrs.insert("_has_curr".to_string(), Value::bool_(false));
}

/// Pull the `itertools` class named `name` out of this module's `module()`
/// and build a `PyInstance` of it carrying `attrs`, bypassing `__init__`.
/// Used by `groupby.__next__` to mint a `_grouper` seeded with private
/// state (parent back-reference, target key, group id).
fn make_itertools_instance(name: &str, attrs: IndexMap<String, Value>) -> Result<Value> {
    let module_val = module();
    let ValueKind::PyModule(m) = module_val.kind() else {
        return Err(PyError::Runtime(
            "internal: itertools module() did not return a PyModule".to_string(),
        ));
    };
    let class_val = m
        .borrow()
        .attrs
        .get(name)
        .cloned()
        .ok_or_else(|| PyError::Runtime(format!("internal: itertools class {name} missing")))?;
    let ValueKind::PyClass(class) = class_val.kind() else {
        return Err(PyError::Runtime(format!(
            "internal: itertools {name} is not a PyClass"
        )));
    };
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(&class),
        attrs,
    }))))
}

/// Read an `_indices`/`_cycles` attribute back as `Vec<usize>`.
fn read_indices(
    inst: &Rc<std::cell::RefCell<PyInstance>>,
    name: &str,
    fn_name: &str,
) -> Result<Vec<usize>> {
    let v = inst
        .borrow()
        .attrs
        .get(name)
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    match v.kind() {
        ValueKind::List(items) => items
            .iter()
            .map(|x| match x.kind() {
                ValueKind::Int(n) => Ok(n as usize),
                _ => Err(internal(fn_name)),
            })
            .collect(),
        _ => Err(internal(fn_name)),
    }
}

fn write_indices(inst: &Rc<std::cell::RefCell<PyInstance>>, name: &str, indices: &[usize]) {
    inst.borrow_mut().attrs.insert(
        name.to_string(),
        Value::list(indices.iter().map(|&i| Value::int(i as i64)).collect()),
    );
}

/// product.__next__ outcome — built under the inst borrow and consumed
/// after the borrow drops to update state without aliasing the cell.
enum ProductStep {
    Yield {
        tuple: Vec<Value>,
        indices: Vec<usize>,
        already_started: bool,
    },
    Exhausted,
}

/// Build a tuple by reading `pools[i][indices[i]]` from a `[Value]`
/// holding nested lists.  Per-element clone is unavoidable (each Value
/// gets returned by value), but no whole-pool clone happens.
fn tuple_from_pools(pools: &[Value], indices: &[usize]) -> Vec<Value> {
    indices
        .iter()
        .enumerate()
        .map(|(i, &j)| match pools[i].kind() {
            ValueKind::List(items) => items[j].clone(),
            _ => Value::none(),
        })
        .collect()
}

/// Build a tuple by reading `pool[indices[k]]` for `k in 0..take`.
fn tuple_from_pool(pool: &[Value], indices: &[usize], take: usize) -> Vec<Value> {
    indices.iter().take(take).map(|&i| pool[i].clone()).collect()
}

// ── combinations / combinations_with_replacement shared algorithm ────────────

fn init_combo_state(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    with_replacement: bool,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded(fn_name, user)?;
    if user.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument 'iterable' (pos 1)"),
        ));
    }
    if user.len() == 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument 'r' (pos 2)"),
        ));
    }
    if user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes at most 2 arguments ({} given)", user.len()),
        ));
    }
    // `collect_iterable` walks generators / __iter__ classes.
    let pool: Vec<Value> = interp.collect_iterable(&user[0].value)?;
    // `r` honors the `__index__` protocol (#2022): a non-int raises the
    // canonical TypeError, a bigint that overflows Py_ssize_t raises
    // OverflowError, and a negative int raises `ValueError: r must be
    // non-negative` — matching CPython 3.12's distinctions.
    let r_i64 = interp.value_to_isize(
        &user[1].value,
        "Python int too large to convert to C ssize_t",
    )?;
    if r_i64 < 0 {
        return Err(PyError::named("ValueError", "r must be non-negative".to_string()));
    }
    let r = r_i64 as usize;
    // For combinations (no replacement), `r > pool.len()` yields nothing.
    let n = pool.len();
    let exhausted = !with_replacement && r > n;
    // Initial indices: all zeros (for replacement) or 0..r (without).
    let indices: Vec<usize> = if with_replacement {
        vec![0; r]
    } else {
        (0..r).collect()
    };
    let mut a = inst.borrow_mut();
    a.attrs.insert("_pool".to_string(), Value::list(pool));
    a.attrs.insert("_r".to_string(), Value::int(r as i64));
    a.attrs.insert(
        "_indices".to_string(),
        Value::list(indices.iter().map(|&i| Value::int(i as i64)).collect()),
    );
    a.attrs.insert("_started".to_string(), Value::bool_(false));
    a.attrs.insert("_exhausted".to_string(), Value::bool_(exhausted));
    Ok(Value::none())
}

fn advance_combinations(
    args: &[ExpandedCallArg],
    fn_name: &str,
    with_replacement: bool,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    enum Outcome {
        Yield { tuple: Vec<Value>, indices: Vec<usize>, set_started: bool },
        EmptyTuple,
        Exhausted,
    }
    check_not_exhausted(&inst, "_exhausted")?;
    // Single immutable borrow: read pool slice (no clone), r, started,
    // indices; build the tuple by direct index lookup.
    let outcome: Outcome = {
        let attrs = inst.borrow();
        let pool_items = match attrs.attrs.get("_pool").map(|v| v.kind()) {
            Some(ValueKind::List(items)) => items,
            _ => return Err(internal(fn_name)),
        };
        let r = match attrs.attrs.get("_r").map(|v| v.kind()) {
            Some(ValueKind::Int(n)) => n as usize,
            _ => return Err(internal(fn_name)),
        };
        let n = pool_items.len();
        let started = matches!(
            attrs.attrs.get("_started").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        );
        // Edge case: r == 0 yields exactly one empty tuple, then stops.
        if r == 0 {
            if started {
                Outcome::Exhausted
            } else {
                Outcome::EmptyTuple
            }
        } else if n == 0 {
            // Empty pool, r > 0 — only with_replacement path can hit
            // this; no-replacement marks _exhausted at init.
            Outcome::Exhausted
        } else {
            let mut indices = read_indices(&inst, "_indices", fn_name)?;
            if started {
                // Find rightmost index that can still grow.
                let mut i = r;
                let exhausted = loop {
                    if i == 0 {
                        break true;
                    }
                    i -= 1;
                    let max_val = if with_replacement { n - 1 } else { n - r + i };
                    if indices[i] < max_val {
                        indices[i] += 1;
                        for j in (i + 1)..r {
                            indices[j] = if with_replacement {
                                indices[i]
                            } else {
                                indices[j - 1] + 1
                            };
                        }
                        break false;
                    }
                };
                if exhausted {
                    Outcome::Exhausted
                } else {
                    Outcome::Yield {
                        tuple: tuple_from_pool(&pool_items[..], &indices, r),
                        indices,
                        set_started: false,
                    }
                }
            } else {
                Outcome::Yield {
                    tuple: tuple_from_pool(&pool_items[..], &indices, r),
                    indices,
                    set_started: true,
                }
            }
        }
    };
    match outcome {
        Outcome::Yield { tuple, indices, set_started } => {
            if set_started {
                inst.borrow_mut()
                    .attrs
                    .insert("_started".to_string(), Value::bool_(true));
            }
            write_indices(&inst, "_indices", &indices);
            Ok(Value::tuple(tuple))
        }
        Outcome::EmptyTuple => {
            inst.borrow_mut()
                .attrs
                .insert("_started".to_string(), Value::bool_(true));
            Ok(Value::tuple(Vec::new()))
        }
        Outcome::Exhausted => {
            inst.borrow_mut()
                .attrs
                .insert("_exhausted".to_string(), Value::bool_(true));
            Err(PyError::named("StopIteration", String::new()))
        }
    }
}
