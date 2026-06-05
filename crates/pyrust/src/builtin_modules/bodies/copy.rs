// `copy` module — `copy.copy` (shallow copy) and `copy.deepcopy` (deep copy).
//
// ## Design
//
// `Value::clone` in pyrust shares the backing `Rc` for mutable containers
// (list, set, dict) — i.e. `b = a` aliases rather than copies.  To produce
// an independent shallow copy we must allocate a new container with the same
// top-level elements (which are themselves shared by reference).
//
// Immutable types (int, float, str, bool, None, bytes, tuple, frozenset)
// are safe to return as-is — CPython does the same (copy.copy of an immutable
// returns the original object).
//
// For `PyInstance`:
//   - If the class defines `__copy__` / `__deepcopy__`, call it.
//   - Otherwise apply the default copy protocol: capture state via
//     `__getstate__()` (defaulting to the instance `__dict__`), build a bare
//     instance of the same class, and restore via `__setstate__(state)`
//     (defaulting to `__dict__.update(state)`).  This mirrors CPython's
//     `object.__reduce_ex__` reduction so `__getstate__` / `__setstate__`
//     are honoured (#2131).
//
// `deepcopy` recurses into the elements of mutable containers.  A `memo` dict
// keyed by object identity (`id(x)`) tracks already-copied objects so that
// cyclic structures terminate (no native stack overflow) and shared
// references stay shared — the copy of `x` is inserted into the memo *before*
// recursing into its children (#1997).  The same `memo` is forwarded to any
// `__deepcopy__(self, memo)` dunder calls so user code receives a proper dict.
//
// Reference: <https://docs.python.org/3/library/copy.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{invoke_class_method, key_to_value, lookup_class_attr, ExpandedCallArg};
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, PyKey, PySet, Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── copy.Error class singleton ────────────────────────────────────────────────

thread_local! {
    static COPY_ERROR_CLASS: Rc<RefCell<PyClass>> = {
        let exception_base = crate::interpreter::lookup_exc_class("Exception")
            .expect("EXC_CLASS_CACHE must contain Exception");
        let class = Rc::new(RefCell::new(PyClass::new(
            "Error",
            "Error",
            Some(Rc::clone(&exception_base)),
            IndexMap::new(),
        )));
        exception_base.borrow().subclasses.borrow_mut().push(Rc::downgrade(&class));
        class
    };
}

fn copy_error_class_value() -> Value {
    COPY_ERROR_CLASS.with(|c| Value::py_class(Rc::clone(c)))
}

pyrust_module! {
    constants {
        // copy.Error — subclass of Exception, raised on deepcopy failures.
        "Error" => copy_error_class_value(),
        // CPython's `copy.py` keeps a lowercase alias for backward
        // compatibility: `error = Error`.  Both names must resolve to the
        // same class so `copy.error is copy.Error` is `True`.
        "error" => copy_error_class_value(),
    }
    /// CPython: copy.copy(x) — shallow copy.
    ///
    /// Immutable types (int, float, str, bool, None, bytes, tuple, frozenset)
    /// are returned as-is.  Mutable containers (list, dict, set) get a new
    /// top-level container with the same elements.  For `PyInstance`, calls
    /// `__copy__` if defined, otherwise applies the default copy protocol
    /// (`__getstate__` / `__setstate__`).
    /// <https://docs.python.org/3/library/copy.html#copy.copy>
    fn copy(args) -> Result<Value> {
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly 1 argument ({} given)", args.len()),
            ));
        }
        let obj = args[0].value.clone();
        shallow_copy(obj, _interp)
    }

    /// CPython: copy.deepcopy(x, memo=None) — deep copy.
    ///
    /// Immutable types are returned as-is.  Mutable containers are recursively
    /// copied.  The `memo` dict (keyed by `id`) terminates cycles and preserves
    /// shared references, and is forwarded to `__deepcopy__` calls.
    /// <https://docs.python.org/3/library/copy.html#copy.deepcopy>
    fn deepcopy(args) -> Result<Value> {
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() takes 1 or 2 arguments ({} given)",
                    args.len()
                ),
            ));
        }
        let obj = args[0].value.clone();
        // Use the caller-supplied memo dict when present; otherwise start with
        // an empty dict.  CPython always passes a dict (never None) to
        // __deepcopy__, so we must forward a proper dict here.
        let memo = if args.len() == 2 {
            args[1].value.clone()
        } else {
            Value::dict(PyDict::default())
        };
        deep_copy(obj, &memo, _interp)
    }
}

// ── identity / memo helpers ───────────────────────────────────────────────────

/// Object identity used as the `memo` key (`id(x)`).  Mirrors the `id()`
/// builtin: instances/classes/modules/functions use their `Rc` address;
/// containers (list/dict/set/tuple) and other Rc-backed values use
/// `Value::value_id`.  Returns `None` for atomic values that have no stable
/// per-object identity worth memoising (those are returned as-is anyway).
fn value_identity(obj: &Value) -> Option<i64> {
    match obj.kind() {
        ValueKind::PyInstance(rc) => Some(Rc::as_ptr(&rc) as i64),
        ValueKind::PyClass(rc) => Some(Rc::as_ptr(&rc) as i64),
        ValueKind::PyModule(rc) => Some(Rc::as_ptr(&rc) as i64),
        ValueKind::UserFunction(rc) => Some(Rc::as_ptr(&rc) as i64),
        _ => obj.value_id(),
    }
}

/// Did `deep_copy(orig)` leave the element unchanged (CPython's `k is j`)?
/// Objects with a stable identity compare equal by that identity; atomic
/// immutables (no `value_identity`) are returned as-is by `deep_copy`, so they
/// always count as unchanged.  Used by the tuple arm to decide whether to
/// return the original tuple (immutable-of-immutables) instead of a fresh one.
fn deepcopy_kept_identity(orig: &Value, copied: &Value) -> bool {
    match (value_identity(orig), value_identity(copied)) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

/// Look up an already-copied object in the memo by identity.
fn memo_get(memo: &Value, id: i64) -> Option<Value> {
    memo.dict_with(|d| d.get(&PyKey::Int(id)).cloned())
        .flatten()
}

/// Record `copy` as the deep copy of the object with identity `id`.
fn memo_insert(memo: &Value, id: i64, copy: Value) {
    let _ = memo.dict_insert(PyKey::Int(id), copy);
}

// ── copy protocol (__getstate__ / __setstate__) ───────────────────────────────

/// Capture the copyable state of the instance behind `rc`.  If the class
/// defines `__getstate__`, call it; otherwise the default state is the
/// instance `__dict__` as a Python `dict` (or `None` when empty, matching
/// CPython 3.12's `object.__getstate__`).
fn capture_state(
    rc: &Rc<RefCell<PyInstance>>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    let class = Rc::clone(&rc.borrow().class);
    if let Some(method) = lookup_class_attr(&class, "__getstate__") {
        return invoke_class_method(interp, method, Value::py_instance(Rc::clone(rc)), &[]);
    }
    // Default: a dict snapshot of __dict__, or None when empty.
    let borrow = rc.borrow();
    if borrow.attrs.is_empty() {
        return Ok(Value::none());
    }
    let mut dict: PyDict = PyDict::with_capacity_and_hasher(borrow.attrs.len(), Default::default());
    for (k, v) in borrow.attrs.iter() {
        dict.insert(PyKey::str_from(k.as_ref()), v.clone());
    }
    Ok(Value::dict(dict))
}

/// Restore captured `state` onto the bare instance `rc`.  If the class defines
/// `__setstate__`, call it; otherwise default to `__dict__.update(state)`.
fn restore_state(
    rc: &Rc<RefCell<PyInstance>>,
    state: Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<()> {
    let class = Rc::clone(&rc.borrow().class);
    if let Some(method) = lookup_class_attr(&class, "__setstate__") {
        invoke_class_method(
            interp,
            method,
            Value::py_instance(Rc::clone(rc)),
            &[ExpandedCallArg {
                name: None,
                value: state,
            }],
        )?;
        return Ok(());
    }
    // Default object.__setstate__ semantics:
    //   - state is None            → no-op
    //   - state is a dict          → __dict__.update(state)
    //   - state is (dict, slotdict)→ apply both as attributes (pyrust has no
    //     __slots__ storage, so slot state becomes ordinary attributes too)
    match state.kind() {
        ValueKind::None => {}
        ValueKind::Tuple(items) if items.len() == 2 => {
            let dict_state = items[0].clone();
            let slot_state = items[1].clone();
            apply_state_dict(rc, &dict_state);
            apply_state_dict(rc, &slot_state);
        }
        _ => apply_state_dict(rc, &state),
    }
    Ok(())
}

/// Apply the str-keyed entries of `state` (when it is a dict) as attributes of
/// the instance behind `rc`.  Mirrors `__dict__.update(state)` — only string
/// keys become attribute names; non-dict / None state is a no-op.
fn apply_state_dict(rc: &Rc<RefCell<PyInstance>>, state: &Value) {
    let pairs: Option<Vec<(PyKey, Value)>> =
        state.dict_with(|d| d.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    if let Some(pairs) = pairs {
        let mut borrow = rc.borrow_mut();
        for (k, v) in pairs {
            if let PyKey::Str(s) = &k
                && let Some(name) = s.as_str() {
                    borrow.attrs.insert(name, v);
                }
        }
    }
}

// ── shallow_copy ──────────────────────────────────────────────────────────────

/// Produce a shallow copy of `obj`.  Immutable types are returned as-is;
/// mutable containers get a new top-level allocation with shared elements.
fn shallow_copy(obj: Value, interp: &mut crate::interpreter::Interpreter) -> Result<Value> {
    match obj.kind() {
        // Immutable scalars — return the same value.
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::Range { .. } => Ok(obj.clone()),

        // Immutable sequences / sets — return the same value.
        // Tuple: CPython's copy.copy returns the same object for tuples.
        ValueKind::Tuple(_) => Ok(obj.clone()),

        // frozenset is immutable — same object.
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "frozenset" => {
            Ok(obj.clone())
        }

        // list — new list with the same elements (shallow: element Values share Rc).
        ValueKind::List(items) => {
            let new_items: Vec<Value> = items.iter().cloned().collect();
            // Drop the Ref borrow guard before constructing the new Value.
            drop(items);
            Ok(Value::list(new_items))
        }

        // dict — new dict with the same key-value pairs.
        ValueKind::Dict(d) => {
            let new_dict: PyDict = d.clone();
            drop(d);
            Ok(Value::dict(new_dict))
        }

        // set — new set with the same keys.
        ValueKind::Set(items) => {
            let new_items: PySet = items.clone();
            drop(items);
            Ok(Value::set(new_items))
        }

        // PyInstance — check for __copy__ dunder first (MRO-aware).
        ValueKind::PyInstance(rc) => {
            let rc = Rc::clone(rc);
            // Look up __copy__ on the class via MRO so inherited dunders
            // are found.  Do not hold any borrow across the call.
            let copy_method = {
                let borrow = rc.borrow();
                let class = Rc::clone(&borrow.class);
                drop(borrow);
                lookup_class_attr(&class, "__copy__")
            };
            if let Some(method) = copy_method {
                // Call `__copy__(self)` — use invoke_class_method so that
                // UserFunction and BuiltinFunction are both handled and
                // `self` is bound via the bound_prefix slot.
                invoke_class_method(
                    interp,
                    method,
                    Value::py_instance(Rc::clone(&rc)),
                    &[],
                )
            } else {
                // Default: copy protocol — capture state via __getstate__,
                // build a bare instance, restore via __setstate__.  When the
                // class customises neither hook this is a verbatim __dict__
                // copy (capture returns the dict, restore re-applies it).
                shallow_copy_via_protocol(&rc, interp)
            }
        }

        // Anything else (functions, classes, modules, generators, …) — return
        // as-is, matching CPython's behaviour for non-copyable objects that don't
        // define __copy__ (they are returned unchanged).
        _ => Ok(obj.clone()),
    }
}

/// Default shallow-copy path for an instance: capture state, build a bare
/// instance of the same class, restore state.  Shallow — the state values are
/// shared by reference, not recursively copied.
fn shallow_copy_via_protocol(
    rc: &Rc<RefCell<PyInstance>>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    let class = Rc::clone(&rc.borrow().class);
    let state = capture_state(rc, interp)?;
    let new_rc = Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    }));
    restore_state(&new_rc, state, interp)?;
    Ok(Value::py_instance(new_rc))
}

// ── deep_copy ─────────────────────────────────────────────────────────────────

/// Produce a deep copy of `obj`.  Immutable types are returned as-is.
/// Mutable containers are recursively copied.  The `memo` dict (keyed by
/// `id`) terminates cycles and preserves shared references; the copy of a
/// container/instance is inserted into the memo *before* its children are
/// recursed into.  `memo` is forwarded to any `__deepcopy__(self, memo)`
/// dunder calls so user code receives a proper dict.
fn deep_copy(
    obj: Value,
    memo: &Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    // Cycle / sharing short-circuit: if we've already copied this object,
    // return the same copy.  Atomic values (value_identity == None) skip this.
    if let Some(id) = value_identity(&obj)
        && let Some(existing) = memo_get(memo, id) {
            return Ok(existing);
        }

    match obj.kind() {
        // Immutable scalars.
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::Range { .. } => Ok(obj.clone()),

        // frozenset — CPython deep-copies the elements even though the
        // container itself is immutable; user-defined hashable objects inside
        // the frozenset may have __deepcopy__ methods.  Extract the inner
        // PySet, convert each key to a Value, deep-copy it, then
        // convert back via value_to_pykey and rebuild a new frozenset.
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "frozenset" => {
            let items_rc = pyrust_builtins::frozenset::as_items(&obj)
                .expect("frozenset arm: as_items must succeed");
            // Snapshot before borrowing mutably through interp.
            let keys: Vec<PyKey> = items_rc.iter().cloned().collect();
            drop(items_rc);
            let mut new_set: PySet = PySet::with_capacity_and_hasher(keys.len(), Default::default());
            for k in keys {
                let v = key_to_value(k);
                let deep_v = deep_copy(v, memo, interp)?;
                let new_k = interp.value_to_pykey(&deep_v)?;
                new_set.insert(new_k);
            }
            let result = pyrust_builtins::frozenset::frozenset(new_set);
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, result.clone());
            }
            Ok(result)
        }

        // tuple — CPython's `_deepcopy_tuple` deep-copies each element, then:
        //   1. re-checks the memo: if a child's recursion cycled back into this
        //      tuple and already produced a copy, return *that* copy (so cyclic
        //      structures rooted at a tuple stay consistent);
        //   2. if every element deep-copied to an identity-unchanged object,
        //      returns the *original* tuple (immutable-of-immutables → same
        //      object, matching `deepcopy((1,2,3)) is (1,2,3)`);
        //   3. otherwise builds a fresh tuple and memoises it.
        ValueKind::Tuple(items) => {
            // Collect to owned Vec first so the borrow (&[Value]) is released
            // before we recursively call deep_copy (which may re-enter match).
            let items_vec: Vec<Value> = items.to_vec();
            // items is &[Value] — a raw reference, no Ref guard to drop.
            let mut new_items = Vec::with_capacity(items_vec.len());
            let mut all_same = true;
            for item in &items_vec {
                let copied = deep_copy(item.clone(), memo, interp)?;
                if !deepcopy_kept_identity(item, &copied) {
                    all_same = false;
                }
                new_items.push(copied);
            }
            // A child may have cycled back and inserted a copy of this tuple
            // under our identity while we recursed — honour it.
            if let Some(id) = value_identity(&obj)
                && let Some(existing) = memo_get(memo, id) {
                    return Ok(existing);
                }
            if all_same {
                // Every element is the same object as in the source → CPython
                // returns the original tuple unchanged.
                return Ok(obj.clone());
            }
            let result = Value::tuple(new_items);
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, result.clone());
            }
            Ok(result)
        }

        // list — new list with deeply-copied elements.  Insert the empty list
        // into the memo *before* recursing so self-referential lists terminate
        // and shared sublists stay shared.
        ValueKind::List(items) => {
            // Collect before dropping the Ref borrow.
            let items_vec: Vec<Value> = items.iter().cloned().collect();
            drop(items);
            let new_list = Value::list(Vec::with_capacity(items_vec.len()));
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, new_list.clone());
            }
            for item in items_vec {
                let copied = deep_copy(item, memo, interp)?;
                new_list.list_push(copied)?;
            }
            Ok(new_list)
        }

        // dict — new dict with deeply-copied keys and values.  Insert the empty
        // dict into the memo before recursing so cyclic dicts terminate.
        ValueKind::Dict(d) => {
            let pairs: Vec<(PyKey, Value)> =
                d.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            drop(d);
            let new_dict =
                Value::dict(PyDict::with_capacity_and_hasher(pairs.len(), Default::default()));
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, new_dict.clone());
            }
            for (k, v) in pairs {
                let deep_k = {
                    let kv = key_to_value(k);
                    let deep_kv = deep_copy(kv, memo, interp)?;
                    interp.value_to_pykey(&deep_kv)?
                };
                let deep_v = deep_copy(v, memo, interp)?;
                let _ = new_dict.dict_insert(deep_k, deep_v);
            }
            Ok(new_dict)
        }

        // set — elements are PyKey but PyKey::Object holds user instances that
        // may define __deepcopy__; CPython deep-copies set elements.  Convert
        // each key to a Value, deep-copy it, then convert back via
        // value_to_pykey and rebuild a new independent set.
        ValueKind::Set(items) => {
            let keys: Vec<PyKey> = items.iter().cloned().collect();
            drop(items);
            let new_set_val = Value::set(PySet::default());
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, new_set_val.clone());
            }
            let mut new_set: PySet = PySet::with_capacity_and_hasher(keys.len(), Default::default());
            for k in keys {
                let v = key_to_value(k);
                let deep_v = deep_copy(v, memo, interp)?;
                let new_k = interp.value_to_pykey(&deep_v)?;
                new_set.insert(new_k);
            }
            new_set_val.set_with_mut(|s| *s = new_set).ok_or_else(|| {
                PyError::named("TypeError", "deepcopy: set rebuild failed".to_string())
            })?;
            Ok(new_set_val)
        }

        // PyInstance — check for __deepcopy__ dunder first (MRO-aware).
        ValueKind::PyInstance(rc) => {
            let rc = Rc::clone(rc);
            // Look up __deepcopy__ on the class via MRO so inherited
            // dunders are found.  Do not hold any borrow across the call.
            let deepcopy_method = {
                let borrow = rc.borrow();
                let class = Rc::clone(&borrow.class);
                drop(borrow);
                lookup_class_attr(&class, "__deepcopy__")
            };
            if let Some(method) = deepcopy_method {
                // Call `__deepcopy__(self, memo)` — forward the memo dict so
                // user code receives a proper dict argument (CPython never
                // passes None here).  invoke_class_method binds `self` via
                // bound_prefix and appends the remaining args after.
                let result = invoke_class_method(
                    interp,
                    method,
                    Value::py_instance(Rc::clone(&rc)),
                    &[ExpandedCallArg {
                        name: None,
                        value: memo.clone(),
                    }],
                )?;
                if let Some(id) = value_identity(&obj) {
                    memo_insert(memo, id, result.clone());
                }
                Ok(result)
            } else {
                deep_copy_via_protocol(&rc, &obj, memo, interp)
            }
        }

        // Anything else — return as-is.
        _ => Ok(obj.clone()),
    }
}

/// Default deep-copy path for an instance: build a bare instance, memoise it
/// *before* deep-copying its state, then capture/restore the state.  Inserting
/// into the memo first lets self-referential instance graphs terminate.
fn deep_copy_via_protocol(
    rc: &Rc<RefCell<PyInstance>>,
    obj: &Value,
    memo: &Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    let class = Rc::clone(&rc.borrow().class);
    let new_rc = Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    }));
    let new_val = Value::py_instance(Rc::clone(&new_rc));
    if let Some(id) = value_identity(obj) {
        memo_insert(memo, id, new_val.clone());
    }
    // Capture state from the original, deep-copy it, then restore.
    let state = capture_state(rc, interp)?;
    let deep_state = deep_copy(state, memo, interp)?;
    restore_state(&new_rc, deep_state, interp)?;
    Ok(new_val)
}
