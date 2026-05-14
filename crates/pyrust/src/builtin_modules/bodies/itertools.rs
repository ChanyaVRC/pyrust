// `itertools` module — body for the `itertools` entry in
// `pyrust_builtin_modules!`.  Currently exposes: `chain`, `islice`,
// `count`, `repeat`, `cycle`, `takewhile`, `dropwhile`, `starmap`,
// `accumulate`, `product`, `combinations`, `combinations_with_replacement`,
// `permutations`, `groupby`.
//
// Still missing vs CPython: `pairwise`, `tee`, `compress`,
// `filterfalse`, `zip_longest`, and `batched` (3.12+).  Tracked in
// the follow-up to #330.
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
use pyrust_derive::pyrust_module;
use std::rc::Rc;

pyrust_module! {
    /// CPython: itertools.chain(*iterables) — concatenate iterables.
    /// Lazy across sources (each is materialised only when reached).
    /// <https://docs.python.org/3/library/itertools.html#itertools.chain>
    fn chain(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let sources: Vec<Value> = args.iter().map(|a| a.value.clone()).collect();
        Ok(pyrust_builtins::iter_helpers::chain(sources))
    }

    /// `itertools.islice` — fully lazy slice with class-based dispatch.
    /// State: `_iter` (advanced past start), `_remaining_stop` (Optional
    /// remaining count until stop), `_step`.
    /// <https://docs.python.org/3/library/itertools.html#itertools.islice>
    class islice {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.is_empty() || user.len() > 4 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes between 2 and 4 arguments"),
                ));
            }
            let (start, stop, step) = match user.len() {
                1 => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes at least 2 arguments"),
                )),
                2 => (0i64, slice_arg(FN_NAME, &user[1].value, "stop")?, 1i64),
                3 => (
                    slice_arg(FN_NAME, &user[1].value, "start")?.unwrap_or(0),
                    slice_arg(FN_NAME, &user[2].value, "stop")?,
                    1i64,
                ),
                4 => (
                    slice_arg(FN_NAME, &user[1].value, "start")?.unwrap_or(0),
                    slice_arg(FN_NAME, &user[2].value, "stop")?,
                    slice_arg(FN_NAME, &user[3].value, "step")?.unwrap_or(1),
                ),
                _ => unreachable!("guarded above"),
            };
            if start < 0 || step <= 0 || stop.is_some_and(|s| s < 0) {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "{FN_NAME}() arguments must be non-negative integers (and step > 0)",
                    ),
                ));
            }
            let iter = make_iter(_interp, user[0].value.clone())?;
            for _ in 0..start {
                match _interp.call_next(iter.clone(), None) {
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

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (iter, remaining, step) = read_islice_state(&inst, FN_NAME)?;
            if let Some(r) = remaining
                && r <= 0
            {
                return Err(PyError::named("StopIteration", String::new()));
            }
            let item = _interp.call_next(iter.clone(), None)?;
            let mut tail_exhausted = false;
            for _ in 0..(step - 1) {
                match _interp.call_next(iter.clone(), None) {
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
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes at most 2 arguments"),
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

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
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
    }

    /// CPython: itertools.repeat(object[, times]) — yield `object`
    /// `times` times, or forever if `times` is *omitted*.  An explicit
    /// `None` is rejected as `TypeError` (matching CPython — `None`
    /// isn't an integer).
    /// <https://docs.python.org/3/library/itertools.html#itertools.repeat>
    class repeat {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 or 2 arguments"),
                ));
            }
            let object = user[0].value.clone();
            let times: Option<i64> = match user.get(1).map(|a| a.value.kind()) {
                None => None,
                Some(ValueKind::Int(n)) => Some(n),
                Some(ValueKind::Bool(b)) => Some(b as i64),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() times argument must be an integer"),
                )),
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

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
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
    }

    /// CPython: itertools.cycle(iterable) — yield iterable's elements,
    /// then repeat forever from a remembered copy.
    /// <https://docs.python.org/3/library/itertools.html#itertools.cycle>
    class cycle {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let iter = make_iter(_interp, user[0].value.clone())?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter".to_string(), iter);
            // `_cache` accumulates each yielded element during the first
            // pass; once `_iter` exhausts, we walk `_cache` indefinitely.
            a.attrs.insert("_cache".to_string(), Value::list(Vec::new()));
            a.attrs.insert("_pos".to_string(), Value::int(0));
            a.attrs.insert("_first_pass".to_string(), Value::bool_(true));
            Ok(Value::none())
        }

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
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
                match _interp.call_next(iter, None) {
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
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 2 arguments"),
                ));
            }
            let iter = make_iter(_interp, user[1].value.clone())?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_pred".to_string(), user[0].value.clone());
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert("_done".to_string(), Value::bool_(false));
            Ok(Value::none())
        }

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if matches!(
                inst.borrow().attrs.get("_done").map(|v| v.kind()),
                Some(ValueKind::Bool(true))
            ) {
                return Err(PyError::named("StopIteration", String::new()));
            }
            let (pred, iter) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_pred").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                )
            };
            let item = _interp.call_next(iter, None)?;
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
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 2 arguments"),
                ));
            }
            let iter = make_iter(_interp, user[1].value.clone())?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_pred".to_string(), user[0].value.clone());
            a.attrs.insert("_iter".to_string(), iter);
            // `_started` flips True once we've seen the first non-matching
            // element; from then on we just drain `_iter` unconditionally.
            a.attrs.insert("_started".to_string(), Value::bool_(false));
            Ok(Value::none())
        }

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
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
                return _interp.call_next(iter, None);
            }
            let pred = inst
                .borrow()
                .attrs
                .get("_pred")
                .cloned()
                .ok_or_else(|| internal(FN_NAME))?;
            // Drain while predicate true.
            loop {
                let item = _interp.call_next(iter.clone(), None)?;
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
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 2 arguments"),
                ));
            }
            let iter = make_iter(_interp, user[1].value.clone())?;
            let mut a = inst.borrow_mut();
            a.attrs.insert("_func".to_string(), user[0].value.clone());
            a.attrs.insert("_iter".to_string(), iter);
            Ok(Value::none())
        }

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
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
            let pack = _interp.call_next(iter, None)?;
            // Unpack — any iterable, including generators and instances
            // with `__iter__`/`__next__`.  `collect_iterable` drives the
            // iterator protocol, unlike the bare `iter_values` helper
            // which only handles built-in containers.
            let unpacked: Vec<ExpandedCallArg> = _interp
                .collect_iterable(pack)?
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
                        format!("{FN_NAME}() got an unexpected keyword argument '{other}'"),
                    )),
                    None => positional.push(a.value.clone()),
                }
            }
            if positional.is_empty() || positional.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 or 2 positional arguments"),
                ));
            }
            let iter = make_iter(_interp, positional[0].clone())?;
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

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
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
                let first = _interp.call_next(iter, None)?;
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
            let nxt = _interp.call_next(iter, None)?;
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
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // `repeat` is keyword-only.
            let mut positional: Vec<Value> = Vec::new();
            let mut repeat: i64 = 1;
            for a in &args[1..] {
                match a.name.as_deref() {
                    Some("repeat") => match a.value.kind() {
                        ValueKind::Int(n) => repeat = n,
                        ValueKind::Bool(b) => repeat = b as i64,
                        _ => return Err(PyError::named(
                            "TypeError",
                            format!("{FN_NAME}() repeat must be an integer"),
                        )),
                    },
                    Some(other) => return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() got an unexpected keyword argument '{other}'"),
                    )),
                    None => positional.push(a.value.clone()),
                }
            }
            if repeat < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("{FN_NAME}() repeat must be non-negative"),
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
                .map(|v| _interp.collect_iterable(v.clone()))
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

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            // One immutable borrow does all the reading: pool lengths,
            // current indices, and the element at each index for the
            // outgoing tuple.  No full-pool clone (the old
            // `read_list_of_lists` path was O(total input size) per
            // yield).
            let outcome: ProductStep = {
                let attrs = inst.borrow();
                if matches!(
                    attrs.attrs.get("_exhausted").map(|v| v.kind()),
                    Some(ValueKind::Bool(true))
                ) {
                    return Err(PyError::named("StopIteration", String::new()));
                }
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
                            tuple: tuple_from_pools(pools_outer, &indices),
                            indices,
                            already_started: true,
                        }
                    }
                } else {
                    ProductStep::Yield {
                        tuple: tuple_from_pools(pools_outer, &indices),
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
        fn __init__(args) -> Result<Value> {
            init_combo_state(_interp, args, FN_NAME, /* with_replacement = */ false)
        }

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }

        fn __next__(args) -> Result<Value> {
            advance_combinations(args, FN_NAME, /* with_replacement = */ false)
        }
    }

    /// CPython: itertools.combinations_with_replacement(iterable, r) —
    /// r-length tuples in input order, repeats allowed.
    /// <https://docs.python.org/3/library/itertools.html#itertools.combinations_with_replacement>
    class combinations_with_replacement {
        fn __init__(args) -> Result<Value> {
            init_combo_state(_interp, args, FN_NAME, /* with_replacement = */ true)
        }

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }

        fn __next__(args) -> Result<Value> {
            advance_combinations(args, FN_NAME, /* with_replacement = */ true)
        }
    }

    /// CPython: itertools.permutations(iterable, r=None) — r-length
    /// permutations in lexicographic order.  `r=None` defaults to the
    /// pool size.
    /// <https://docs.python.org/3/library/itertools.html#itertools.permutations>
    class permutations {
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            reject_keyword_args_expanded(FN_NAME, user)?;
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 or 2 arguments"),
                ));
            }
            // `collect_iterable` walks generators / __iter__ classes too.
            let pool: Vec<Value> = _interp.collect_iterable(user[0].value.clone())?;
            // CPython splits negative-r (ValueError) from non-int-r
            // (TypeError); match that so user `except` blocks behave
            // identically.
            let r = match user.get(1).map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => pool.len(),
                Some(ValueKind::Int(n)) if n >= 0 => n as usize,
                Some(ValueKind::Int(_)) => return Err(PyError::named(
                    "ValueError",
                    format!("{FN_NAME}() r must be non-negative"),
                )),
                Some(ValueKind::Bool(b)) => b as usize,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() r must be an integer or None"),
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

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
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
            let outcome: Step = {
                let attrs = inst.borrow();
                if matches!(
                    attrs.attrs.get("_exhausted").map(|v| v.kind()),
                    Some(ValueKind::Bool(true))
                ) {
                    return Err(PyError::named("StopIteration", String::new()));
                }
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
                            tuple: tuple_from_pool(pool_items, &indices, r),
                            indices,
                            cycles,
                            already_started: true,
                        }
                    }
                } else {
                    Step::Yield {
                        tuple: tuple_from_pool(pool_items, &indices, r),
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
    /// *consecutive* equal elements.  Each yield is `(key, group_iter)`.
    /// The group iterator is a list (eagerly materialised on yield) —
    /// CPython returns a lazy view, but materialising is simpler and
    /// the typical idiom `for k, g in groupby(data, key): list(g)`
    /// uses the group fully anyway.
    /// <https://docs.python.org/3/library/itertools.html#itertools.groupby>
    class groupby {
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
                        format!("{FN_NAME}() got an unexpected keyword argument '{other}'"),
                    )),
                    None => positional.push(a.value.clone()),
                }
            }
            if positional.is_empty() || positional.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 or 2 positional arguments"),
                ));
            }
            if positional.len() == 2 && key_kw.is_some() {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() got multiple values for argument 'key'"),
                ));
            }
            let iter = make_iter(_interp, positional[0].clone())?;
            let key_fn = key_kw
                .or_else(|| positional.get(1).cloned())
                .unwrap_or_else(Value::none);
            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert("_keyfn".to_string(), key_fn);
            // `_pending` holds the next element + its key, lookahead
            // style.  Empty until first __next__ pulls.
            a.attrs.insert("_pending".to_string(), Value::none());
            a.attrs.insert("_pending_key".to_string(), Value::none());
            a.attrs.insert("_has_pending".to_string(), Value::bool_(false));
            a.attrs.insert("_exhausted".to_string(), Value::bool_(false));
            Ok(Value::none())
        }

        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }

        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if matches!(
                inst.borrow().attrs.get("_exhausted").map(|v| v.kind()),
                Some(ValueKind::Bool(true))
            ) {
                return Err(PyError::named("StopIteration", String::new()));
            }
            // Read state.
            let (iter, key_fn, has_pending, pending, pending_key) = {
                let a = inst.borrow();
                (
                    a.attrs.get("_iter").cloned().ok_or_else(|| internal(FN_NAME))?,
                    a.attrs.get("_keyfn").cloned().ok_or_else(|| internal(FN_NAME))?,
                    matches!(
                        a.attrs.get("_has_pending").map(|v| v.kind()),
                        Some(ValueKind::Bool(true))
                    ),
                    a.attrs.get("_pending").cloned().unwrap_or_else(Value::none),
                    a.attrs.get("_pending_key").cloned().unwrap_or_else(Value::none),
                )
            };
            // Get the first element of this group.  If we have a
            // pending element from the previous group's lookahead, use
            // it; otherwise pull a new one.
            let (first, first_key) = if has_pending {
                (pending, pending_key)
            } else {
                let item = match _interp.call_next(iter.clone(), None) {
                    Ok(v) => v,
                    Err(e) if is_stop_iteration(&e) => {
                        inst.borrow_mut()
                            .attrs
                            .insert("_exhausted".to_string(), Value::bool_(true));
                        return Err(PyError::named("StopIteration", String::new()));
                    }
                    Err(e) => return Err(e),
                };
                let k = compute_key(_interp, &key_fn, &item)?;
                (item, k)
            };
            // Collect everything else with the same key.
            let mut group: Vec<Value> = vec![first.clone()];
            loop {
                let item = match _interp.call_next(iter.clone(), None) {
                    Ok(v) => v,
                    Err(e) if is_stop_iteration(&e) => {
                        // End-of-iter — stash that we have no more pending.
                        let mut a = inst.borrow_mut();
                        a.attrs.insert("_has_pending".to_string(), Value::bool_(false));
                        a.attrs.insert("_exhausted".to_string(), Value::bool_(true));
                        return Ok(Value::tuple(vec![first_key, Value::list(group)]));
                    }
                    Err(e) => return Err(e),
                };
                let k = compute_key(_interp, &key_fn, &item)?;
                // `==` on Rust-side `Value` is identity-based for
                // PyInstance, so we must go through `eval_binary(Eq)` to
                // dispatch `__eq__` — otherwise `groupby(items, key=K)`
                // would treat every `K(v)` instance as its own group.
                // The result must then go through `truthy_value` so a
                // user `__eq__` returning a PyInstance routes through
                // `__bool__`/`__len__`.
                let eq_val =
                    _interp.eval_binary(k.clone(), crate::ast::BinaryOp::Eq, first_key.clone())?;
                let eq = _interp.truthy_value(&eq_val)?;
                if eq {
                    group.push(item);
                } else {
                    // Stash as pending for the next group.
                    let mut a = inst.borrow_mut();
                    a.attrs.insert("_pending".to_string(), item);
                    a.attrs.insert("_pending_key".to_string(), k);
                    a.attrs.insert("_has_pending".to_string(), Value::bool_(true));
                    return Ok(Value::tuple(vec![first_key, Value::list(group)]));
                }
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
fn slice_arg(fn_name: &str, v: &Value, slot: &str) -> Result<Option<i64>> {
    match v.kind() {
        ValueKind::None => Ok(None),
        ValueKind::Int(n) => Ok(Some(n)),
        ValueKind::Bool(b) => Ok(Some(b as i64)),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() {slot} argument must be an integer or None"),
        )),
    }
}

/// `count.__init__` validation — start/step must be numeric.
/// BigInt is accepted so `count(10**30)` works the same as
/// `count(10)` (matching Python's arbitrary-precision ints); the
/// running `_cur` may then transition between `Int` and `BigInt`
/// as values cross the i64 boundary, but `eval_binary(Add)`
/// handles both directions of that conversion.
fn require_numeric(v: &Value, fn_name: &str, slot: &str) -> Result<()> {
    if matches!(
        v.kind(),
        ValueKind::Int(_) | ValueKind::Float(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
    ) {
        Ok(())
    } else {
        Err(PyError::named(
            "TypeError",
            format!("{fn_name}() {slot} must be a number"),
        ))
    }
}

/// Construct an iterator from an iterable.  Equivalent to Python's
/// `iter(obj)`; we go through the interpreter's `iter` builtin so
/// `__iter__`-providing classes are handled uniformly.
fn make_iter(interp: &mut crate::Interpreter, iterable: Value) -> Result<Value> {
    interp.call_function_expanded(
        Value::builtin_function("iter"),
        &[ExpandedCallArg {
            name: None,
            value: iterable,
        }],
    )
}

/// Recognise StopIteration in both the plain (`PyError::Named`) and
/// exception-instance (`PyError::Raised`) forms.
fn is_stop_iteration(e: &PyError) -> bool {
    match e {
        PyError::Named(name, _) => name == "StopIteration",
        PyError::Raised(exc) => matches!(
            exc.kind(),
            ValueKind::PyInstance(i) if i.borrow().class.borrow().name == "StopIteration"
        ),
        _ => false,
    }
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
    if user.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes 2 arguments"),
        ));
    }
    // `collect_iterable` walks generators / __iter__ classes.
    let pool: Vec<Value> = interp.collect_iterable(user[0].value.clone())?;
    // Same split as `permutations`: negative-r is ValueError, non-int is
    // TypeError — matches CPython's distinction.
    let r = match user[1].value.kind() {
        ValueKind::Int(n) if n >= 0 => n as usize,
        ValueKind::Int(_) => return Err(PyError::named(
            "ValueError",
            format!("{fn_name}() r must be non-negative"),
        )),
        ValueKind::Bool(b) => b as usize,
        _ => return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() r must be an integer"),
        )),
    };
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
    // Single immutable borrow: read pool slice (no clone), r, started,
    // indices; build the tuple by direct index lookup.
    let outcome: Outcome = {
        let attrs = inst.borrow();
        if matches!(
            attrs.attrs.get("_exhausted").map(|v| v.kind()),
            Some(ValueKind::Bool(true))
        ) {
            return Err(PyError::named("StopIteration", String::new()));
        }
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
                        tuple: tuple_from_pool(pool_items, &indices, r),
                        indices,
                        set_started: false,
                    }
                }
            } else {
                Outcome::Yield {
                    tuple: tuple_from_pool(pool_items, &indices, r),
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
