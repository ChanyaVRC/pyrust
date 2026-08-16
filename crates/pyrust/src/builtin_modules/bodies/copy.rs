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
use std::rc::{Rc, Weak};

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, effective_builtin_receiver, invoke_class_method, is_exception_class,
    key_to_value, lookup_class_attr, make_iterator, restore_reduced_iterator_position,
};
use crate::value::{
    InstanceAttrs, PyClass, PyDict, PyInstance, PyKey, PyModule, PySet, Value, ValueKind,
};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

const COPY_PY_SOURCE: &str = include_str!("copy_py.py");

/// Bind the native `copy` fast path to this import generation's private
/// Python reconstruction helper. The helper stays out of the public module
/// namespace and is supplied as a hidden receiver on each `copy()` call.
pub(crate) fn inject_python_members(
    interp: &mut crate::interpreter::Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(COPY_PY_SOURCE, Some(ns.clone()), None)?;
    let helper = ns
        .dict_with(|dict| dict.get(&PyKey::str_from("_reconstruct_shallow")).cloned())
        .flatten()
        .ok_or_else(|| PyError::Runtime("copy: reconstruction helper missing".into()))?;
    let native_copy = module
        .borrow()
        .attrs
        .get("copy")
        .cloned()
        .ok_or_else(|| PyError::Runtime("copy: native copy function missing".into()))?;
    let copy = pyrust_builtins::native_builtin_callable::native_generation_builtin(
        native_copy,
        helper,
        "copy",
        "copy",
    );
    module.borrow_mut().insert_attr("copy".to_string(), copy);
    Ok(Some(ns))
}

// ── copy.Error class generations ──────────────────────────────────────────────

thread_local! {
    /// `copy` is a Python module in CPython, so each fresh import generation
    /// owns a distinct Error class. Weak storage keeps old classes alive only
    /// while an old module/class/instance still references them.
    static COPY_ERROR_CLASSES: RefCell<Vec<Weak<RefCell<PyClass>>>> =
        const { RefCell::new(Vec::new()) };
}

fn new_copy_error_class_value() -> Value {
    let exception_base = crate::interpreter::lookup_exc_class("Exception")
        .expect("EXC_CLASS_CACHE must contain Exception");
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("copy"));
    let class = Rc::new(RefCell::new(PyClass::new(
        "Error",
        "Error",
        Some(Rc::clone(&exception_base)),
        attrs,
    )));
    exception_base
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&class));
    COPY_ERROR_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|registered| registered.strong_count() > 0);
        classes.push(Rc::downgrade(&class));
    });
    Value::py_class(class)
}

fn current_copy_error_class_value() -> Value {
    let class = COPY_ERROR_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|registered| registered.strong_count() > 0);
        classes.iter().rev().find_map(Weak::upgrade)
    });
    class.map_or_else(new_copy_error_class_value, Value::py_class)
}

pyrust_module! {
    constants {
        // copy.Error — subclass of Exception, raised on deepcopy failures.
        "Error" => new_copy_error_class_value(),
        // CPython's `copy.py` keeps a lowercase alias for backward
        // compatibility: `error = Error`.  Both names must resolve to the
        // same class so `copy.error is copy.Error` is `True`.
        "error" => current_copy_error_class_value(),
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
        // The injected public callable supplies its private reconstruction
        // helper as args[0]; only the remaining arguments are user-visible.
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() takes exactly 1 argument ({} given)",
                    args.len().saturating_sub(1)
                ),
            ));
        }
        let reconstruct = args[0].value.clone();
        let obj = args[1].value.clone();
        shallow_copy_with_reconstruct(obj, Some(&reconstruct), _interp)
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
        ValueKind::PyInstance(rc) => Some(Rc::as_ptr(rc) as i64),
        ValueKind::PyClass(rc) => Some(Rc::as_ptr(rc) as i64),
        ValueKind::PyModule(rc) => Some(Rc::as_ptr(rc) as i64),
        ValueKind::UserFunction(rc) => Some(Rc::as_ptr(rc) as i64),
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
    Ok(default_instance_state(rc))
}

/// Snapshot the instance attributes used by the default copy protocol, or
/// return `None` when the instance carries no state.
pub(crate) fn default_instance_state(rc: &Rc<RefCell<PyInstance>>) -> Value {
    // `InstanceAttrs` also carries primitive-subclass storage. Exclude only
    // the exact backing resolved through canonical ancestry; a plain user
    // object may legitimately own an attribute with this spelling.
    let receiver = Value::py_instance(Rc::clone(rc));
    let builtin_backing = effective_builtin_receiver(&receiver, &[]);
    let borrow = rc.borrow();
    let attrs: Vec<_> = borrow
        .attrs
        .items_snapshot()
        .into_iter()
        .filter(|(name, value)| {
            name.as_ref() != crate::interpreter::BUILTIN_DATA_ATTR
                || !builtin_backing
                    .as_ref()
                    .is_some_and(|backing| backing.is_identical_to(value))
        })
        .collect();
    if attrs.is_empty() {
        return Value::none();
    }
    let mut dict: PyDict = PyDict::with_capacity_and_hasher(attrs.len(), Default::default());
    for (k, v) in attrs {
        dict.insert(PyKey::str_from(k.as_ref()), v);
    }
    Value::dict(dict)
}

/// Restore captured `state` onto the bare instance `rc`.  If the class defines
/// `__setstate__`, call it; otherwise default to `__dict__.update(state)`.
fn restore_state(
    rc: &Rc<RefCell<PyInstance>>,
    state: Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<()> {
    // copy._reconstruct applies state only when the reducer supplied one.
    // In particular, an otherwise-stateless subclass must not have its
    // custom `__setstate__` called merely because PyRust stores a backing.
    if state.is_none() {
        return Ok(());
    }
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
                && let Some(name) = s.as_str()
            {
                borrow.attrs.insert(name, v);
            }
        }
    }
}

// ── exception copy protocol (#2360 / #2361) ───────────────────────────────────

/// Reconstruct a copy of an exception instance the way CPython does: via the
/// `BaseException.__reduce__` value `(type, args[, state])`.  The constructor is
/// re-invoked as `type(*args)` (running any user `__init__`), then the non-slot
/// `__dict__` state is re-applied.  The C-level slots — `__traceback__`,
/// `__cause__`, `__context__`, `__suppress_context__` — are deliberately *not*
/// carried over, so the copy starts with a fresh (None) traceback/cause/context,
/// matching CPython 3.12.
///
/// `copy_val` produces the per-element copy (identity for shallow, recursive for
/// deepcopy), so this single path serves both `copy` and `deepcopy`.
///
/// `on_constructed` runs after the new instance is built but *before* the state
/// dict is (deep-)copied onto it — deepcopy uses this hook to insert the new
/// instance into the memo, so a self-referential attribute (`e.x = e`) cycles
/// back to the same copy instead of recursing forever.
fn copy_exception(
    rc: &Rc<RefCell<PyInstance>>,
    interp: &mut crate::interpreter::Interpreter,
    mut copy_val: impl FnMut(Value, &mut crate::interpreter::Interpreter) -> Result<Value>,
    on_constructed: impl FnOnce(&Value),
) -> Result<Value> {
    let cls = crate::interpreter::value_class(&Value::py_instance(Rc::clone(rc)));
    // `self.args` — the constructor arguments.  Copy each element so deepcopy
    // recurses into args (CPython deep-copies args via the state machinery).
    let args_tuple = rc
        .borrow()
        .attrs
        .get_slot("args")
        .cloned()
        .unwrap_or_else(|| Value::tuple(Vec::new()));
    let arg_values: Vec<Value> = match args_tuple.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => Vec::new(),
    };
    let mut ctor_args: Vec<ExpandedCallArg> = Vec::with_capacity(arg_values.len());
    for v in arg_values {
        ctor_args.push(ExpandedCallArg {
            name: None,
            value: copy_val(v, interp)?,
        });
    }
    // Reconstruct via `type(*args)` — re-runs __new__/__init__ like CPython.
    let new_val = interp.call_function_expanded(cls, &ctor_args)?;
    on_constructed(&new_val);
    // Re-apply the non-slot __dict__ state (custom attrs, __notes__, …).
    let state = pyrust_builtins::instance_dict::exception_dict_state(rc);
    if !state.is_empty()
        && let ValueKind::PyInstance(new_rc) = new_val.kind()
    {
        for (k, v) in state {
            let copied = copy_val(v, interp)?;
            new_rc.borrow_mut().attrs.insert(&k, copied);
        }
    }
    Ok(new_val)
}

// ── shallow_copy ──────────────────────────────────────────────────────────────

/// Produce a shallow copy of `obj`.  Immutable types are returned as-is;
/// mutable containers get a new top-level allocation with shared elements.
fn shallow_copy_with_reconstruct(
    obj: Value,
    reconstruct: Option<&Value>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
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
        ValueKind::BuiltinObject { ops, .. }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
        {
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

        // Opaque built-in storage (bytearray, an instance `__dict__` view,
        // a deque's `VecDeque` payload, …).  The type owns the knowledge of
        // how to detach its backing; types that are safe to share (immutable
        // or identity-like) return None and are returned unchanged.
        ValueKind::BuiltinObject { ops, state } => {
            Ok(ops.copy_storage(state).unwrap_or_else(|| obj.clone()))
        }

        // Built-in iterator objects (#2974).  CPython copies each one through
        // its own `__reduce__`, so the iteration domain — which owns every
        // cursor representation — rebuilds the reduce-equivalent state.
        ValueKind::Generator(_) => match crate::interpreter::copy_iterator_object(&obj, false)? {
            crate::interpreter::IteratorCopy::Rebuilt(copy) => Ok(copy),
            crate::interpreter::IteratorCopy::BytearrayReduction { carrier, position } => {
                rebuild_reduced_iterator(carrier, position, interp)
            }
            crate::interpreter::IteratorCopy::GetItemReduction {
                constructor,
                owner,
                position,
            } => rebuild_getitem_reduction(constructor, owner, position, interp),
            crate::interpreter::IteratorCopy::Unpicklable(noun) => Err(PyError::named(
                "TypeError",
                format!("cannot pickle '{noun}' object"),
            )),
            crate::interpreter::IteratorCopy::Unowned => Ok(obj.clone()),
        },

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
            let dynamic_reversed_reduction =
                copy_method.is_none() && crate::interpreter::is_reversed_iterator(&obj);
            let iterator_reduction = if copy_method.is_none() && !dynamic_reversed_reduction {
                match crate::interpreter::copy_iterator_object(&obj, false)? {
                    crate::interpreter::IteratorCopy::GetItemReduction {
                        constructor,
                        owner,
                        position,
                    } => Some((constructor, owner, position)),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(method) = copy_method.filter(|method| {
                !method.is_none()
                    && !pyrust_builtins::classmethod::as_static_method_any(method)
                        .is_some_and(|wrapped| wrapped.is_none())
            }) {
                // Call `__copy__(self)` — use invoke_class_method so that
                // UserFunction and BuiltinFunction are both handled and
                // `self` is bound via the bound_prefix slot.
                //
                // CPython's `copy.copy` is pure Python and resolves the hook as
                // a *class* attribute, then passes the object explicitly:
                // `copier = getattr(cls, "__copy__", None); copier(x)`.  For a
                // plain `def` that class-level lookup is unbound, so the single
                // explicit argument lands on `self` — identical to prepending
                // the receiver.  A `staticmethod` / `classmethod` hook, though,
                // still receives `x` as a real argument *on top of* its own
                // descriptor binding, so it needs one explicit positional
                // (issue #2939 review).  Note `__deepcopy__` is different:
                // `copy.deepcopy` uses an *instance* getattr and passes only
                // `memo`, which the plain binding path already models.
                let instance = Value::py_instance(Rc::clone(&rc));
                let binds_own_receiver = matches!(
                    method.kind(),
                    ValueKind::UserFunction(f)
                        if f.kind != pyrust_core::UserFunctionKind::Regular
                );
                if binds_own_receiver {
                    let call_args = [ExpandedCallArg {
                        name: None,
                        value: instance.clone(),
                    }];
                    invoke_class_method(interp, method, instance, &call_args)
                } else {
                    invoke_class_method(interp, method, instance, &[])
                }
            } else if dynamic_reversed_reduction {
                let reduction = call_copy_reducer(&obj, interp)?;
                if let Some(reconstruct) = reconstruct {
                    interp.call_function_expanded(
                        reconstruct.clone(),
                        &[
                            ExpandedCallArg {
                                name: None,
                                value: obj.clone(),
                            },
                            ExpandedCallArg {
                                name: None,
                                value: reduction,
                            },
                        ],
                    )
                } else {
                    reconstruct_reduction_shallow(&obj, reduction, interp)
                }
            } else if let Some((constructor, owner, position)) = iterator_reduction {
                rebuild_getitem_reduction(constructor, owner, position, interp)
            } else if let Some(reconstruct) = reconstruct
                && let Some(reduce_method) = static_reduce_method(&rc)
            {
                let instance = Value::py_instance(Rc::clone(&rc));
                let reduction = invoke_class_method(interp, reduce_method, instance.clone(), &[])?;
                interp.call_function_expanded(
                    reconstruct.clone(),
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: instance,
                        },
                        ExpandedCallArg {
                            name: None,
                            value: reduction,
                        },
                    ],
                )
            } else if is_exception_class(&rc.borrow().class) {
                // #2360 / #2361: exceptions copy via their `__reduce__` value —
                // reconstruct `type(*args)` and re-apply the non-slot state.
                // Shallow: share the arg/state values (identity copy).
                copy_exception(&rc, interp, |v, _| Ok(v), |_| {})
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

/// Preserve the pre-#2958 shallow-copy implementation for recursive storage
/// detachment, which never needs the module-generation reconstruction helper.
fn shallow_copy(obj: Value, interp: &mut crate::interpreter::Interpreter) -> Result<Value> {
    shallow_copy_with_reconstruct(obj, None, interp)
}

/// Return the class-level staticmethod `__reduce__` only when the inherited
/// canonical object reducer is unquestionably the active `__reduce_ex__`.
/// Custom instance hooks and `__getattribute__` stay on the pre-existing
/// fallback path tracked by #2953.
fn static_reduce_method(instance: &Rc<RefCell<PyInstance>>) -> Option<Value> {
    let class = Rc::clone(&instance.borrow().class);
    {
        let instance = instance.borrow();
        if instance.attrs.get("__reduce__").is_some() {
            return None;
        }
        if instance
            .attrs
            .get("__reduce_ex__")
            .is_some_and(|method| !method.is_none())
        {
            return None;
        }
    }
    if lookup_class_attr(&class, "__getattribute__").is_some_and(|method| {
        !crate::interpreter::value_is_canonical_slot(
            &method,
            crate::interpreter::CanonicalSlot::ObjectGetAttribute,
        )
    }) {
        return None;
    }
    let reduce_ex_falls_through = lookup_class_attr(&class, "__reduce_ex__").is_none_or(|method| {
        method.is_none()
            || pyrust_builtins::classmethod::as_static_method_any(&method)
                .is_some_and(|wrapped| wrapped.is_none())
            || crate::interpreter::value_is_canonical_slot(
                &method,
                crate::interpreter::CanonicalSlot::ObjectReduceEx,
            )
            || crate::interpreter::value_is_canonical_slot(
                &method,
                crate::interpreter::CanonicalSlot::BaseExceptionReduceEx,
            )
    });
    if !reduce_ex_falls_through {
        return None;
    }
    lookup_class_attr(&class, "__reduce__").filter(|method| {
        matches!(
            method.kind(),
            ValueKind::UserFunction(function)
                if function.kind == pyrust_core::UserFunctionKind::StaticMethod
        ) || pyrust_builtins::classmethod::as_static_method_any(method).is_some()
    })
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
    detach_internal_storage(rc, &new_rc, interp, shallow_copy)?;
    Ok(Value::py_instance(new_rc))
}

/// Is this attribute the instance's own built-in payload rather than one of
/// its element values?  Two forms exist: the `__builtin_data__` backing every
/// `dict` / `list` / `set` / `bytearray` / … subclass instance carries, and an
/// opaque storage object a built-in class parks in a named attribute (a
/// `deque`'s `VecDeque`).  Both are storage the copy must own outright.
fn is_internal_storage_attr(name: &str, value: &Value) -> bool {
    name == crate::interpreter::BUILTIN_DATA_ATTR
        || matches!(value.kind(), ValueKind::BuiltinObject { ops, .. } if ops.is_internal_storage())
}

/// Give a freshly built shallow copy its *own* built-in payload (#2935).
///
/// A built-in-backed instance — `OrderedDict`, `Counter`, `defaultdict`,
/// `deque`, and every user subclass of `dict` / `list` / `set` / `bytearray` —
/// keeps its contents in an internal attribute.  That storage is the object's
/// own state, not a shared element value, so leaving the attribute pointing at
/// the original's backing makes writes to the copy rewrite the original.
/// Shallow-copying the backing keeps CPython's semantics: the structure is
/// independent while the values inside stay shared.
///
/// The storage is taken from the *original* rather than from the restored
/// state so that a class customising `__getstate__` can neither alias nor drop
/// its own backing.  Dropping it is not a milder failure than aliasing it: a
/// `dict` subclass instance with no `__builtin_data__` is not an empty mapping
/// but a broken one, and `len()` on it recurses until the native stack
/// overflows.  Both `copy` and `deepcopy` therefore run this step —
/// `copy_val` supplies the per-storage copy (identity-preserving for shallow,
/// memo-aware recursion for deep).
///
/// For the common case (no `__getstate__`) this is idempotent: the state
/// snapshot already carried the backing, so the copy re-derived here is the
/// very value `restore_state` installed — under `deepcopy` the memo returns it
/// verbatim rather than building a second one.
fn detach_internal_storage(
    rc: &Rc<RefCell<PyInstance>>,
    new_rc: &Rc<RefCell<PyInstance>>,
    interp: &mut crate::interpreter::Interpreter,
    mut copy_val: impl FnMut(Value, &mut crate::interpreter::Interpreter) -> Result<Value>,
) -> Result<()> {
    let storages: Vec<(String, Value)> = rc
        .borrow()
        .attrs
        .items_snapshot()
        .into_iter()
        .filter(|(name, value)| is_internal_storage_attr(name.as_ref(), value))
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    for (name, storage) in storages {
        let independent = copy_val(storage, interp)?;
        new_rc.borrow_mut().attrs.insert(&name, independent);
    }
    Ok(())
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
        && let Some(existing) = memo_get(memo, id)
    {
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
        ValueKind::BuiltinObject { ops, .. }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
        {
            let items_rc = pyrust_builtins::frozenset::as_items(&obj)
                .expect("frozenset arm: as_items must succeed");
            // Snapshot before borrowing mutably through interp.
            let keys: Vec<PyKey> = items_rc.iter().cloned().collect();
            drop(items_rc);
            let mut new_set: PySet =
                PySet::with_capacity_and_hasher(keys.len(), Default::default());
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
                && let Some(existing) = memo_get(memo, id)
            {
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
            let new_dict = Value::dict(PyDict::with_capacity_and_hasher(
                pairs.len(),
                Default::default(),
            ));
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
            let mut new_set: PySet =
                PySet::with_capacity_and_hasher(keys.len(), Default::default());
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

        // Opaque built-in storage (bytearray, an instance `__dict__` view, a
        // deque's `VecDeque` payload, …).  Detach the backing through the
        // type's own hook, then recurse into the payload it holds — the
        // detached storage is memoised first so a cycle running back through
        // its elements terminates.
        ValueKind::BuiltinObject { ops, state } => {
            let Some(copied) = ops.copy_storage(state) else {
                return Ok(obj.clone());
            };
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, copied.clone());
            }
            if let Some(elements) = ops.storage_elements(state) {
                let mut deep_elements = Vec::with_capacity(elements.len());
                for element in elements {
                    deep_elements.push(deep_copy(element, memo, interp)?);
                }
                if let ValueKind::BuiltinObject {
                    ops: copied_ops,
                    state: copied_state,
                } = copied.kind()
                {
                    copied_ops.set_storage_elements(copied_state, deep_elements);
                }
            } else if let Some(pairs) = copied.dict_with(|d| {
                d.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            }) {
                // A storage that detaches into an ordinary container: an
                // instance `__dict__` copies into a plain `dict` whose keys are
                // attribute names (always `str`, so only the values recurse).
                for (key, value) in pairs {
                    let deep_value = deep_copy(value, memo, interp)?;
                    let _ = copied.dict_insert(key, deep_value);
                }
            }
            Ok(copied)
        }

        // Built-in iterator objects (#2974).  Rebuild the reduce-equivalent
        // cursor, memoise it, then deep-copy the sources it retained — the same
        // two-step the opaque-storage arm above uses, so an iterator reachable
        // from its own source terminates instead of recursing.
        ValueKind::Generator(_) => {
            let rebuilt = match crate::interpreter::copy_iterator_object(&obj, true)? {
                crate::interpreter::IteratorCopy::Rebuilt(copy) => copy,
                crate::interpreter::IteratorCopy::BytearrayReduction { carrier, position } => {
                    // CPython copies reduction arguments before invoking the
                    // reducer. A carrier that points back to this iterator
                    // therefore reconstructs a second iterator over the
                    // already-memoised copied carrier, rather than aliasing
                    // the outer copy.
                    let copied_carrier = deep_copy(carrier, memo, interp)?;
                    let copy = rebuild_reduced_iterator(copied_carrier, position, interp)?;
                    if let Some(id) = value_identity(&obj) {
                        memo_insert(memo, id, copy.clone());
                    }
                    return Ok(copy);
                }
                crate::interpreter::IteratorCopy::GetItemReduction {
                    constructor,
                    owner,
                    position,
                } => {
                    // `copy._reconstruct` deep-copies the reduction arguments
                    // before it invokes the reducer or memoises the outer
                    // iterator. This ordering deliberately creates a distinct
                    // inner cursor when the owner points back to the iterator.
                    let copied_owner = deep_copy(owner, memo, interp)?;
                    let copy = construct_getitem_reduction(constructor, copied_owner, interp)?;
                    if let Some(id) = value_identity(&obj) {
                        memo_insert(memo, id, copy.clone());
                    }
                    apply_getitem_reduction_state(&copy, position, interp)?;
                    return Ok(copy);
                }
                crate::interpreter::IteratorCopy::Unpicklable(noun) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("cannot pickle '{noun}' object"),
                    ));
                }
                crate::interpreter::IteratorCopy::Unowned => return Ok(obj.clone()),
            };
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, rebuilt.clone());
            }
            if let Some(sources) = crate::interpreter::iterator_retained_values(&rebuilt) {
                let mut deep_sources = Vec::with_capacity(sources.len());
                for source in sources {
                    deep_sources.push(deep_copy(source, memo, interp)?);
                }
                crate::interpreter::set_iterator_retained_values(&rebuilt, deep_sources)?;
            }
            Ok(rebuilt)
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
            let dynamic_reversed_reduction =
                deepcopy_method.is_none() && crate::interpreter::is_reversed_iterator(&obj);
            let iterator_reduction = if deepcopy_method.is_none() && !dynamic_reversed_reduction {
                match crate::interpreter::copy_iterator_object(&obj, true)? {
                    crate::interpreter::IteratorCopy::GetItemReduction {
                        constructor,
                        owner,
                        position,
                    } => Some((constructor, owner, position)),
                    _ => None,
                }
            } else {
                None
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
            } else if dynamic_reversed_reduction {
                let reduction = call_copy_reducer(&obj, interp)?;
                reconstruct_reduction_deep(&obj, reduction, memo, interp)
            } else if let Some((constructor, owner, position)) = iterator_reduction {
                let copied_owner = deep_copy(owner, memo, interp)?;
                let copy = construct_getitem_reduction(constructor, copied_owner, interp)?;
                if let Some(id) = value_identity(&obj) {
                    memo_insert(memo, id, copy.clone());
                }
                apply_getitem_reduction_state(&copy, position, interp)?;
                Ok(copy)
            } else if is_exception_class(&rc.borrow().class) {
                // #2360 / #2361: exceptions deep-copy via their `__reduce__`
                // value — reconstruct `type(*args)` (recursing into args) and
                // re-apply the deep-copied non-slot state.  __traceback__ /
                // __cause__ / __context__ are excluded, matching CPython.
                let obj_id = value_identity(&obj);
                copy_exception(
                    &rc,
                    interp,
                    |v, interp| deep_copy(v, memo, interp),
                    |new_val| {
                        // Memoise *before* the state recurses so a
                        // self-referential attr (`e.x = e`) cycles back here.
                        if let Some(id) = obj_id {
                            memo_insert(memo, id, new_val.clone());
                        }
                    },
                )
            } else {
                deep_copy_via_protocol(&rc, &obj, memo, interp)
            }
        }

        // Anything else — return as-is.
        _ => Ok(obj.clone()),
    }
}

/// Execute a bytearray iterator's `(iter, (carrier,), position)` reduction.
/// A carrier subclass may override `__iter__`, changing the concrete type of
/// the copied iterator; the reduction result, not the old native frame, owns
/// that decision.
fn rebuild_reduced_iterator(
    carrier: Value,
    position: usize,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    let rebuilt = make_iterator(interp, &carrier)?;
    apply_getitem_reduction_state(&rebuilt, Some(position as i64), interp)?;
    Ok(rebuilt)
}

/// Execute a legacy sequence cursor's typed reduction. The constructor call is
/// intentionally dynamic: `iter(copied_owner)`, `reversed(copied_owner)`, or a
/// `reversed` subclass may resolve to a different iterator type than the
/// source cursor did. State is then applied to that result exactly as
/// `copy._reconstruct` would.
fn rebuild_getitem_reduction(
    constructor: Value,
    owner: Value,
    position: Option<i64>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    let rebuilt = construct_getitem_reduction(constructor, owner, interp)?;
    apply_getitem_reduction_state(&rebuilt, position, interp)?;
    Ok(rebuilt)
}

fn construct_getitem_reduction(
    constructor: Value,
    owner: Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    interp.call_function_expanded(
        constructor,
        &[ExpandedCallArg {
            name: None,
            value: owner,
        }],
    )
}

/// Resolve the same reducer that CPython's copy module asks an instance for.
/// This is used only for genuine `reversed` subclasses, after their explicit
/// copy hook has had priority; ordinary instances retain the existing copy
/// protocol path.
fn call_copy_reducer(obj: &Value, interp: &mut crate::interpreter::Interpreter) -> Result<Value> {
    match interp.get_attr(obj, "__reduce_ex__") {
        Ok(reducer) if !reducer.is_none() => {
            return interp.call_function_expanded(
                reducer,
                &[ExpandedCallArg {
                    name: None,
                    value: Value::int(4),
                }],
            );
        }
        Ok(_) => {}
        Err(error) if error.class_name_is("AttributeError") => {}
        Err(error) => return Err(error),
    }
    let reducer = interp.get_attr(obj, "__reduce__")?;
    interp.call_function_expanded(reducer, &[])
}

fn reduction_parts(reduction: Value) -> Result<Vec<Value>> {
    reduction
        .as_tuple()
        .map(|parts| parts.to_vec())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "copy reduction must return a string or tuple".to_string(),
            )
        })
}

fn reduction_pair(
    entry: &Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<[Value; 2]> {
    let iterator = make_iterator(interp, entry)?;
    let first = match interp.call_next(&iterator, None) {
        Ok(value) => value,
        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => {
            return Err(pyrust_core::value_err!(
                "not enough values to unpack (expected 2, got 0)"
            ));
        }
        Err(error) => return Err(error),
    };
    let second = match interp.call_next(&iterator, None) {
        Ok(value) => value,
        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => {
            return Err(pyrust_core::value_err!(
                "not enough values to unpack (expected 2, got 1)"
            ));
        }
        Err(error) => return Err(error),
    };
    match interp.call_next(&iterator, None) {
        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => Ok([first, second]),
        Ok(_) => Err(pyrust_core::value_err!(
            "too many values to unpack (expected 2)"
        )),
        Err(error) => Err(error),
    }
}

fn reconstruct_reduction_shallow(
    original: &Value,
    reduction: Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    if reduction.as_str().is_some() {
        return Ok(original.clone());
    }
    let parts = reduction_parts(reduction)?;
    if !(2..=3).contains(&parts.len()) {
        return Err(PyError::named(
            "TypeError",
            format!("copy reduction has invalid length {}", parts.len()),
        ));
    }
    let args = parts[1].as_tuple().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "copy reduction args must be a tuple".to_string(),
        )
    })?;
    let call_args: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    let rebuilt = interp.call_function_expanded(parts[0].clone(), &call_args)?;
    if let Some(state) = parts.get(2).filter(|state| !state.is_none()) {
        apply_reduction_state(&rebuilt, state.clone(), interp)?;
    }
    Ok(rebuilt)
}

fn reconstruct_reduction_deep(
    original: &Value,
    reduction: Value,
    memo: &Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    if reduction.as_str().is_some() {
        return Ok(original.clone());
    }
    let parts = reduction_parts(reduction)?;
    if !(2..=5).contains(&parts.len()) {
        return Err(PyError::named(
            "TypeError",
            format!("copy reduction has invalid length {}", parts.len()),
        ));
    }
    let args = parts[1].as_tuple().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "copy reduction args must be a tuple".to_string(),
        )
    })?;
    let mut call_args = Vec::with_capacity(args.len());
    for value in args {
        call_args.push(ExpandedCallArg {
            name: None,
            value: deep_copy(value.clone(), memo, interp)?,
        });
    }
    let rebuilt = interp.call_function_expanded(parts[0].clone(), &call_args)?;
    if let Some(id) = value_identity(original) {
        memo_insert(memo, id, rebuilt.clone());
    }
    if let Some(state) = parts.get(2).filter(|state| !state.is_none()) {
        let copied_state = deep_copy(state.clone(), memo, interp)?;
        apply_deep_reduction_state(&rebuilt, copied_state, interp)?;
    }
    if let Some(listiter) = parts.get(3).filter(|listiter| !listiter.is_none()) {
        let iterator = make_iterator(interp, listiter)?;
        loop {
            let item = match interp.call_next(&iterator, None) {
                Ok(item) => item,
                Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
                Err(error) => return Err(error),
            };
            let copied_item = deep_copy(item, memo, interp)?;
            let append = interp.get_attr(&rebuilt, "append")?;
            interp.call_function_expanded(
                append,
                &[ExpandedCallArg {
                    name: None,
                    value: copied_item,
                }],
            )?;
        }
    }
    if let Some(dictiter) = parts.get(4).filter(|dictiter| !dictiter.is_none()) {
        let iterator = make_iterator(interp, dictiter)?;
        loop {
            let entry = match interp.call_next(&iterator, None) {
                Ok(entry) => entry,
                Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
                Err(error) => return Err(error),
            };
            let [key, value] = reduction_pair(&entry, interp)?;
            let copied_key = deep_copy(key, memo, interp)?;
            let copied_value = deep_copy(value, memo, interp)?;
            let setitem = interp.get_attr(&rebuilt, "__setitem__")?;
            interp.call_function_expanded(
                setitem,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: copied_key,
                    },
                    ExpandedCallArg {
                        name: None,
                        value: copied_value,
                    },
                ],
            )?;
        }
    }
    Ok(rebuilt)
}

fn apply_deep_reduction_state(
    rebuilt: &Value,
    state: Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<()> {
    match interp.get_attr(rebuilt, "__setstate__") {
        Ok(setstate) => {
            interp.call_function_expanded(
                setstate,
                &[ExpandedCallArg {
                    name: None,
                    value: state,
                }],
            )?;
        }
        Err(error) if error.class_name_is("AttributeError") => {
            let (dict_state, slot_state) = match state.as_tuple() {
                Some(parts) if parts.len() == 2 => (parts[0].clone(), Some(parts[1].clone())),
                _ => (state, None),
            };
            if !dict_state.is_none() {
                let dict = interp.get_attr(rebuilt, "__dict__")?;
                let update = interp.get_attr(&dict, "update")?;
                interp.call_function_expanded(
                    update,
                    &[ExpandedCallArg {
                        name: None,
                        value: dict_state,
                    }],
                )?;
            }
            if let Some(slot_state) = slot_state.filter(|slot_state| !slot_state.is_none()) {
                let items = interp.get_attr(&slot_state, "items")?;
                let entries = interp.call_function_expanded(items, &[])?;
                let iterator = make_iterator(interp, &entries)?;
                loop {
                    let entry = match interp.call_next(&iterator, None) {
                        Ok(entry) => entry,
                        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
                        Err(error) => return Err(error),
                    };
                    let [name, value] = reduction_pair(&entry, interp)?;
                    let name = name.as_str().ok_or_else(|| {
                        pyrust_core::type_err!(
                            "attribute name must be string, not '{}'",
                            crate::interpreter::full_type_name_str(&name)
                        )
                    })?;
                    interp.assign_attr(rebuilt.clone(), name, value)?;
                }
            }
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn apply_getitem_reduction_state(
    rebuilt: &Value,
    position: Option<i64>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<()> {
    let Some(position) = position else {
        return Ok(());
    };
    if restore_reduced_iterator_position(rebuilt, position)? {
        return Ok(());
    }
    apply_reduction_state(rebuilt, Value::int(position), interp)
}

fn apply_reduction_state(
    rebuilt: &Value,
    state: Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<()> {
    match interp.get_attr(rebuilt, "__setstate__") {
        Ok(setstate) => {
            interp.call_function_expanded(
                setstate,
                &[ExpandedCallArg {
                    name: None,
                    value: state,
                }],
            )?;
        }
        // `copy._reconstruct` uses `hasattr(y, "__setstate__")`, then falls
        // back to `y.__dict__.update(state)`.  Preserve that fallback for an
        // override returning an arbitrary iterator: it intentionally raises
        // TypeError for our integer reduction state on a normal instance and
        // AttributeError for a generator, which has no `__dict__`.
        Err(error) if error.class_name_is("AttributeError") => {
            let dict = interp.get_attr(rebuilt, "__dict__")?;
            let update = interp.get_attr(&dict, "update")?;
            interp.call_function_expanded(
                update,
                &[ExpandedCallArg {
                    name: None,
                    value: state,
                }],
            )?;
        }
        Err(error) => return Err(error),
    }
    Ok(())
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
    // A bytearray subclass's copied reduction carrier can be asked for an
    // iterator while its user state is still recursing (the source may point
    // back to the original iterator). Seed its primitive buffer first, as
    // CPython's bytearray reconstruction does, so that nested `iter(copy)` is
    // well-defined and retains the same live buffer. Other built-in-backed
    // instances keep the established restore-then-detach order.
    let bytearray_backed = effective_builtin_receiver(obj, &[])
        .is_some_and(|backing| pyrust_builtins::bytearray::as_bytearray_rc(&backing).is_some());
    if bytearray_backed {
        detach_internal_storage(rc, &new_rc, interp, |value, interp| {
            deep_copy(value, memo, interp)
        })?;
    }
    // Capture state from the original, deep-copy it, then restore.
    let state = capture_state(rc, interp)?;
    let deep_state = deep_copy(state, memo, interp)?;
    restore_state(&new_rc, deep_state, interp)?;
    if !bytearray_backed {
        detach_internal_storage(rc, &new_rc, interp, |value, interp| {
            deep_copy(value, memo, interp)
        })?;
    }
    Ok(new_val)
}
