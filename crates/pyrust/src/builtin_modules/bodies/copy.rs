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

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, invoke_class_method, is_exception_class, key_to_value, lookup_class_attr,
    make_iterator, restore_reduced_iterator_position,
};
use crate::value::{
    InstanceAttrs, PyClass, PyDict, PyInstance, PyKey, PyModule, PySet, Value, ValueKind,
};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

const COPY_PY_SOURCE: &str = include_str!("copy_py.py");

/// Bind the native `copy` and `deepcopy` fast paths to this import
/// generation's private Python reconstruction helpers. The helpers stay out of
/// the public module namespace and are supplied as hidden receivers on each
/// call.
pub(crate) fn inject_python_members(
    interp: &mut crate::interpreter::Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(COPY_PY_SOURCE, Some(ns.clone()), None)?;
    let helper_shallow = ns
        .dict_with(|dict| dict.get(&PyKey::str_from("_reconstruct_shallow")).cloned())
        .flatten()
        .ok_or_else(|| PyError::Runtime("copy: shallow reconstruction helper missing".into()))?;
    let helper_deep = ns
        .dict_with(|dict| dict.get(&PyKey::str_from("_reconstruct_deep")).cloned())
        .flatten()
        .ok_or_else(|| PyError::Runtime("copy: deep reconstruction helper missing".into()))?;
    let native_copy = module
        .borrow()
        .attrs
        .get("copy")
        .cloned()
        .ok_or_else(|| PyError::Runtime("copy: native copy function missing".into()))?;
    let native_deepcopy = module
        .borrow()
        .attrs
        .get("deepcopy")
        .cloned()
        .ok_or_else(|| PyError::Runtime("copy: native deepcopy function missing".into()))?;
    let copy = pyrust_builtins::native_builtin_callable::native_generation_builtin(
        native_copy,
        helper_shallow,
        "copy",
        "copy",
    );
    let deepcopy = pyrust_builtins::native_builtin_callable::native_generation_builtin(
        native_deepcopy,
        helper_deep,
        "deepcopy",
        "copy",
    );
    {
        let mut module_mut = module.borrow_mut();
        module_mut.insert_attr("copy".to_string(), copy);
        module_mut.insert_attr("deepcopy".to_string(), deepcopy);
    }
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
        // The injected public callable supplies its private reconstruction
        // helper as args[0]; only the remaining arguments are user-visible.
        let reconstruct_deep = args[0].value.clone();
        let user_args = &args[1..];
        if user_args.is_empty() || user_args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() takes 1 or 2 arguments ({} given)",
                    user_args.len()
                ),
            ));
        }
        let obj = user_args[0].value.clone();
        // Use the caller-supplied memo dict when present and not None;
        // otherwise start with an empty dict.  CPython always passes a dict
        // (never None) to __deepcopy__, so we must forward a proper dict here.
        let memo = if user_args.len() == 2 && !user_args[1].value.is_none() {
            user_args[1].value.clone()
        } else {
            Value::dict(PyDict::default())
        };
        deep_copy_with_reconstruct(obj, &memo, Some(&reconstruct_deep), _interp)
    }
}

// ── identity / memo helpers ───────────────────────────────────────────────────

/// Object identity used as the `memo` key (`id(x)`).  Uses the single canonical
/// `Value::object_id` encoding shared with Python's `id()` and `is`, so
/// identity-proxied built-in views such as `obj.__dict__` (`InstanceDict`)
/// memoise under a stable identity distinct from `obj` itself (#2946).
/// Returns `None` for atomic scalar values that are returned as-is anyway.
fn value_identity(obj: &Value) -> Option<i64> {
    match obj.kind() {
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::Float(_)
        | ValueKind::Complex(_, _) => None,
        _ => obj.object_id().as_int(),
    }
}

/// Did `deep_copy(orig)` leave the element unchanged (CPython's `k is j`)?
fn deepcopy_kept_identity(orig: &Value, copied: &Value) -> bool {
    orig.is_identical_to(copied)
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

/// Snapshot the instance attributes and `__slots__` member slots used by the
/// default copy protocol / `object.__getstate__`, or return `None` when the
/// instance carries no state.
///
/// Built-in subclass backing (`builtin_data`) and native C-style slots
/// (`_cursor`, `_items`, `maxlen`, `default_factory`, …) live in physically
/// separate `InstanceAttrs` storage outside the Python-visible mapping (#2968).
pub(crate) fn default_instance_state(rc: &Rc<RefCell<PyInstance>>) -> Value {
    crate::interpreter::default_instance_state(rc)
}

/// Restore captured `state` onto the bare instance `rc`.  If the class defines
/// `__setstate__`, call it; otherwise default to `__dict__.update(state)` (and
/// `setattr` for slot state when `state` is a `(dict_state, slot_state)` pair).
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
    //   - state is (dict, slotdict)→ update __dict__ from first element,
    //     apply second element via `setattr` so `__slots__` land in member_slots.
    match state.kind() {
        ValueKind::None => {}
        ValueKind::Tuple(items) if items.len() == 2 => {
            let dict_state = items[0].clone();
            let slot_state = items[1].clone();
            apply_state_dict(rc, &dict_state);
            apply_slot_state(rc, &slot_state, interp)?;
        }
        _ => apply_state_dict(rc, &state),
    }
    Ok(())
}

/// Apply the str-keyed entries of `slot_state` via `assign_attr` so `__slots__`
/// descriptors populate `member_slots` while unslotted classes still populate
/// `__dict__`.
fn apply_slot_state(
    rc: &Rc<RefCell<PyInstance>>,
    slot_state: &Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<()> {
    let pairs: Option<Vec<(String, Value)>> = slot_state.dict_with(|d| {
        d.iter()
            .filter_map(|(k, v)| match k {
                PyKey::Str(s) => s.as_str().map(|name| (name.to_string(), v.clone())),
                _ => None,
            })
            .collect()
    });
    if let Some(pairs) = pairs {
        for (name, val) in pairs {
            interp.assign_attr(Value::py_instance(Rc::clone(rc)), &name, val)?;
        }
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
            if pyrust_builtins::mapping_proxy::is_mapping_proxy(&obj) {
                return Err(PyError::named(
                    "TypeError",
                    "cannot pickle 'mappingproxy' object".to_string(),
                ));
            }
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
            crate::interpreter::IteratorCopy::Unowned => {
                let mut id_copy = |v: Value| Ok(v);
                let mut no_memo_get = |_: i64| None;
                let mut no_memo_insert = |_: i64, _: Value| {};
                Ok(crate::interpreter::copy_provider_iterator(
                    &obj,
                    false,
                    &mut id_copy,
                    &mut no_memo_get,
                    &mut no_memo_insert,
                    None,
                )?
                .unwrap_or_else(|| obj.clone()))
            }
        },

        // PyInstance — CPython 3.12 copy.copy precedence:
        //   1. class-level `__copy__` (`getattr(cls, "__copy__", None)`)
        //   2. `__reduce_ex__(4)` / `__reduce__()` (falling back to the fast
        //      default copy protocol when the canonical inherited reducer has
        //      no user override)
        //   3. `copy.Error` when both reducers are disabled (`None`/missing)
        ValueKind::PyInstance(rc) => {
            let rc = Rc::clone(rc);
            let class = Rc::clone(&rc.borrow().class);
            let cls_val = Value::py_class(Rc::clone(&class));
            match interp.get_attr(&cls_val, "__copy__") {
                Ok(copier) if !copier.is_none() => {
                    return interp.call_function_expanded(
                        copier,
                        &[ExpandedCallArg {
                            name: None,
                            value: obj.clone(),
                        }],
                    );
                }
                Ok(_) => {}
                Err(error) if error.class_name_is("AttributeError") => {}
                Err(error) => return Err(error),
            }
            match resolve_instance_copy_reduction(&obj, &class, false, interp)? {
                InstanceReduction::Reduced(reduction) => {
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
                }
                InstanceReduction::DefaultProtocol => {
                    if let crate::interpreter::IteratorCopy::GetItemReduction {
                        constructor,
                        owner,
                        position,
                    } = crate::interpreter::copy_iterator_object(&obj, false)?
                    {
                        rebuild_getitem_reduction(constructor, owner, position, interp)
                    } else if is_exception_class(&class) {
                        // #2360 / #2361: exceptions copy via their `__reduce__`
                        // value — reconstruct `type(*args)` and re-apply the
                        // non-slot state. Shallow: share arg/state values.
                        copy_exception(&rc, interp, |v, _| Ok(v), |_| {})
                    } else if crate::builtin_modules::itertools::is_unpicklable_itertools_class(
                        &class,
                    ) {
                        let noun = pyrust_core::error_type_name(&obj);
                        Err(PyError::named(
                            "TypeError",
                            format!("cannot pickle '{noun}' object"),
                        ))
                    } else if let Some(result) = {
                        let mut id_copy = |v: Value, _: &mut crate::interpreter::Interpreter| Ok(v);
                        let mut no_memo = |_: i64, _: Value| {};
                        crate::builtin_modules::itertools::copy_groupby_or_grouper(
                            &rc,
                            interp,
                            &mut id_copy,
                            &mut no_memo,
                            None,
                        )
                    } {
                        result
                    } else {
                        // Default: copy protocol — capture state via
                        // __getstate__, build a bare instance, restore via
                        // __setstate__, and detach internal built-in storage.
                        shallow_copy_via_protocol(&rc, interp)
                    }
                }
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

enum InstanceReduction {
    /// Inherited canonical reducer with no user override: keep the native
    /// default protocol (exception reconstruction or `__getstate__`/`__setstate__`
    /// plus `#2945` internal built-in storage detachment).
    DefaultProtocol,
    /// A custom `__reduce_ex__` or `__reduce__` hook returned a reduction value
    /// to pass through `_reconstruct`.
    Reduced(Value),
}

fn raise_uncopyable_error(
    class: &Rc<RefCell<PyClass>>,
    is_deep: bool,
    interp: &mut crate::interpreter::Interpreter,
) -> PyError {
    let cls_val = Value::py_class(Rc::clone(class));
    let cls_repr = crate::interpreter::render_value_repr(interp, &cls_val)
        .unwrap_or_else(|_| cls_val.repr_raw());
    let kind = if is_deep { "deep" } else { "shallow" };
    let message = format!("un({kind})copyable object of type {cls_repr}");
    match current_copy_error_class_value().kind() {
        ValueKind::PyClass(err_cls) => PyError::Class(Rc::clone(err_cls), message),
        _ => PyError::Runtime(message),
    }
}

/// Whether `reductor` is the canonical inherited `object` or `BaseException`
/// reducer bound directly to `obj` (rather than a user override, instance
/// attribute, descriptor, or `__getattribute__` replacement).
fn is_canonical_bound_reducer(
    reductor: &Value,
    obj: &Value,
    class: &Rc<RefCell<PyClass>>,
    attr_name: &str,
) -> bool {
    let Some((method_name, receiver)) = pyrust_builtins::bound_method::as_bound_method(reductor)
    else {
        return false;
    };
    if method_name.as_str() != attr_name || !receiver.is_identical_to(obj) {
        return false;
    }
    let Some(slot) = lookup_class_attr(class, attr_name) else {
        return false;
    };
    match attr_name {
        "__reduce_ex__" => {
            crate::interpreter::value_is_canonical_slot(
                &slot,
                crate::interpreter::CanonicalSlot::ObjectReduceEx,
            ) || crate::interpreter::value_is_canonical_slot(
                &slot,
                crate::interpreter::CanonicalSlot::BaseExceptionReduceEx,
            )
        }
        "__reduce__" => {
            crate::interpreter::value_is_canonical_slot(
                &slot,
                crate::interpreter::CanonicalSlot::ObjectReduce,
            ) || (is_exception_class(class)
                && crate::interpreter::value_is_canonical_slot(
                    &slot,
                    crate::interpreter::CanonicalSlot::BaseExceptionReduce,
                ))
        }
        _ => false,
    }
}

/// Whether `class` overrides `__reduce__` relative to its canonical base
/// (`object.__reduce__`, or `BaseException.__reduce__` for exception classes).
/// Mirrors CPython's `object_reduce_ex` check on `Py_TYPE(self)`.
fn class_overrides_reduce(class: &Rc<RefCell<PyClass>>) -> bool {
    lookup_class_attr(class, "__reduce__").is_some_and(|slot| {
        !crate::interpreter::value_is_canonical_slot(
            &slot,
            crate::interpreter::CanonicalSlot::ObjectReduce,
        ) && !(is_exception_class(class)
            && crate::interpreter::value_is_canonical_slot(
                &slot,
                crate::interpreter::CanonicalSlot::BaseExceptionReduce,
            ))
    })
}

/// Resolve and invoke the `__reduce_ex__(4)` / `__reduce__()` hook on `obj`
/// following CPython 3.12's `Lib/copy.py` and `object_reduce_ex` precedence.
fn resolve_instance_copy_reduction(
    obj: &Value,
    class: &Rc<RefCell<PyClass>>,
    is_deep: bool,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<InstanceReduction> {
    match interp.get_attr(obj, "__reduce_ex__") {
        Ok(reductor) if !reductor.is_none() => {
            if is_canonical_bound_reducer(&reductor, obj, class, "__reduce_ex__") {
                if class_overrides_reduce(class) {
                    let inst_reduce = interp.get_attr(obj, "__reduce__")?;
                    let rv = interp.call_function_expanded(inst_reduce, &[])?;
                    return Ok(InstanceReduction::Reduced(rv));
                }
                return Ok(InstanceReduction::DefaultProtocol);
            }
            let rv = interp.call_function_expanded(
                reductor,
                &[ExpandedCallArg {
                    name: None,
                    value: Value::int(4),
                }],
            )?;
            return Ok(InstanceReduction::Reduced(rv));
        }
        Ok(_) => {}
        Err(error) if error.class_name_is("AttributeError") => {}
        Err(error) => return Err(error),
    }

    match interp.get_attr(obj, "__reduce__") {
        Ok(reductor) if interp.truthy_value(&reductor)? => {
            if is_canonical_bound_reducer(&reductor, obj, class, "__reduce__") {
                return Ok(InstanceReduction::DefaultProtocol);
            }
            let rv = interp.call_function_expanded(reductor, &[])?;
            Ok(InstanceReduction::Reduced(rv))
        }
        Ok(_) => Err(raise_uncopyable_error(class, is_deep, interp)),
        Err(error) if error.class_name_is("AttributeError") => {
            Err(raise_uncopyable_error(class, is_deep, interp))
        }
        Err(error) => Err(error),
    }
}

/// Default shallow-copy path for an instance: capture state, build a bare
/// instance of the same class, restore state, and copy opaque internal storage.
/// Shallow — the state values are shared by reference, not recursively copied.
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
    detach_internal_storage_shallow(rc, &new_rc, interp)?;
    restore_state(&new_rc, state, interp)?;
    Ok(Value::py_instance(new_rc))
}

/// Give a freshly built shallow copy its *own* opaque built-in payload (#2935,
/// #2968): detaches `builtin_data` (`list`/`dict`/`set`/`bytearray`/…) and any
/// internal-storage native slots (`_items` on `deque`, `_cursor` on
/// combinatoric iterators), while preserving scalar/reference native slots such
/// as `deque.maxlen` and `defaultdict.default_factory`.
fn detach_internal_storage_shallow(
    rc: &Rc<RefCell<PyInstance>>,
    new_rc: &Rc<RefCell<PyInstance>>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<()> {
    let (builtin_data, native_slots) = {
        let borrow = rc.borrow();
        (
            borrow.attrs.builtin_data().cloned(),
            borrow.attrs.slots_snapshot(),
        )
    };
    if let Some(backing) = builtin_data {
        let independent = shallow_copy(backing, interp)?;
        new_rc.borrow_mut().attrs.set_builtin_data(independent);
    }
    for (name, value) in native_slots {
        let copied = if matches!(
            value.kind(),
            ValueKind::BuiltinObject { ops, .. } if ops.is_internal_storage()
        ) {
            shallow_copy(value, interp)?
        } else {
            value
        };
        new_rc.borrow_mut().attrs.insert_slot(name.as_ref(), copied);
    }
    Ok(())
}

thread_local! {
    /// Active `copy.deepcopy` recursion depth, guarding against unbounded
    /// native recursion on unmemoized pre-construction cycles such as a
    /// self-referential `Counter` (#2968).
    static DEEPCOPY_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct DeepCopyDepthGuard;

impl DeepCopyDepthGuard {
    fn enter(interp: &crate::interpreter::Interpreter) -> Result<Self> {
        // Cap below the native debug stack ceiling while still honouring
        // smaller user-configured `sys.setrecursionlimit()` values.
        const NATIVE_DEEPCOPY_CAP: usize = 200;
        let limit = crate::interpreter::get_recursion_limit(interp).min(NATIVE_DEEPCOPY_CAP);
        let next = DEEPCOPY_DEPTH.with(|depth| {
            let next = depth.get() + 1;
            if next > limit {
                return Err(PyError::named(
                    "RecursionError",
                    "maximum recursion depth exceeded".to_string(),
                ));
            }
            depth.set(next);
            Ok(next)
        })?;
        let _ = next;
        Ok(Self)
    }
}

impl Drop for DeepCopyDepthGuard {
    fn drop(&mut self) {
        DEEPCOPY_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

// ── deep_copy ─────────────────────────────────────────────────────────────────

/// Produce a deep copy of `obj`.  Immutable types are returned as-is.
/// Mutable containers are recursively copied.  The `memo` dict (keyed by
/// `id`) terminates cycles and preserves shared references; the copy of a
/// container/instance is inserted into the memo *before* its children are
/// recursed into.  `memo` is forwarded to any `__deepcopy__(self, memo)`
/// dunder calls so user code receives a proper dict.
fn deep_copy_with_reconstruct(
    obj: Value,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    let _depth_guard = DeepCopyDepthGuard::enter(interp)?;
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
                let deep_v = deep_copy_with_reconstruct(v, memo, reconstruct, interp)?;
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
                let copied = deep_copy_with_reconstruct(item.clone(), memo, reconstruct, interp)?;
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
                let copied = deep_copy_with_reconstruct(item, memo, reconstruct, interp)?;
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
                    let deep_kv = deep_copy_with_reconstruct(kv, memo, reconstruct, interp)?;
                    interp.value_to_pykey(&deep_kv)?
                };
                let deep_v = deep_copy_with_reconstruct(v, memo, reconstruct, interp)?;
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
                let deep_v = deep_copy_with_reconstruct(v, memo, reconstruct, interp)?;
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
            if pyrust_builtins::mapping_proxy::is_mapping_proxy(&obj) {
                return Err(PyError::named(
                    "TypeError",
                    "cannot pickle 'mappingproxy' object".to_string(),
                ));
            }
            let Some(copied) = ops.copy_storage(state) else {
                return Ok(obj.clone());
            };
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, copied.clone());
            }
            if let Some(elements) = ops.storage_elements(state) {
                let mut deep_elements = Vec::with_capacity(elements.len());
                for element in elements {
                    deep_elements.push(deep_copy_with_reconstruct(
                        element,
                        memo,
                        reconstruct,
                        interp,
                    )?);
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
                    let deep_value = deep_copy_with_reconstruct(value, memo, reconstruct, interp)?;
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
                    let copied_carrier =
                        deep_copy_with_reconstruct(carrier, memo, reconstruct, interp)?;
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
                    let copied_owner =
                        deep_copy_with_reconstruct(owner, memo, reconstruct, interp)?;
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
                crate::interpreter::IteratorCopy::Unowned => {
                    let mut copy_child =
                        |v: Value| deep_copy_with_reconstruct(v, memo, reconstruct, interp);
                    let mut get_memo = |id: i64| memo_get(memo, id);
                    let mut put_memo = |id: i64, v: Value| memo_insert(memo, id, v);
                    return Ok(crate::interpreter::copy_provider_iterator(
                        &obj,
                        true,
                        &mut copy_child,
                        &mut get_memo,
                        &mut put_memo,
                        value_identity(&obj),
                    )?
                    .unwrap_or_else(|| obj.clone()));
                }
            };
            if let Some(id) = value_identity(&obj) {
                memo_insert(memo, id, rebuilt.clone());
            }
            if let Some(sources) = crate::interpreter::iterator_retained_values(&rebuilt) {
                let mut deep_sources = Vec::with_capacity(sources.len());
                for source in sources {
                    deep_sources.push(deep_copy_with_reconstruct(
                        source,
                        memo,
                        reconstruct,
                        interp,
                    )?);
                }
                crate::interpreter::set_iterator_retained_values(&rebuilt, deep_sources)?;
            }
            Ok(rebuilt)
        }

        // PyInstance — CPython 3.12 copy.deepcopy precedence:
        //   1. instance-level `__deepcopy__` (`getattr(x, "__deepcopy__", None)`)
        //   2. `__reduce_ex__(4)` / `__reduce__()` (falling back to the fast
        //      default copy protocol when the canonical inherited reducer has
        //      no user override)
        //   3. `copy.Error` when both reducers are disabled (`None`/missing)
        ValueKind::PyInstance(rc) => {
            let rc = Rc::clone(rc);
            let class = Rc::clone(&rc.borrow().class);
            match interp.get_attr(&obj, "__deepcopy__") {
                Ok(copier) if !copier.is_none() => {
                    let result = interp.call_function_expanded(
                        copier,
                        &[ExpandedCallArg {
                            name: None,
                            value: memo.clone(),
                        }],
                    )?;
                    if !deepcopy_kept_identity(&obj, &result)
                        && let Some(id) = value_identity(&obj)
                    {
                        memo_insert(memo, id, result.clone());
                    }
                    return Ok(result);
                }
                Ok(_) => {}
                Err(error) if error.class_name_is("AttributeError") => {}
                Err(error) => return Err(error),
            }
            match resolve_instance_copy_reduction(&obj, &class, true, interp)? {
                InstanceReduction::Reduced(reduction) => {
                    let rebuilt = if let Some(reconstruct) = reconstruct {
                        let deepcopy_fn =
                            pyrust_builtins::native_builtin_callable::native_generation_builtin(
                                Value::builtin_function("copy.deepcopy"),
                                reconstruct.clone(),
                                "deepcopy",
                                "copy",
                            );
                        interp.call_function_expanded(
                            reconstruct.clone(),
                            &[
                                ExpandedCallArg {
                                    name: None,
                                    value: obj.clone(),
                                },
                                ExpandedCallArg {
                                    name: None,
                                    value: memo.clone(),
                                },
                                ExpandedCallArg {
                                    name: None,
                                    value: reduction,
                                },
                                ExpandedCallArg {
                                    name: None,
                                    value: deepcopy_fn,
                                },
                            ],
                        )?
                    } else {
                        reconstruct_reduction_deep(&obj, reduction, memo, interp)?
                    };
                    if !deepcopy_kept_identity(&obj, &rebuilt)
                        && let Some(id) = value_identity(&obj)
                    {
                        memo_insert(memo, id, rebuilt.clone());
                    }
                    Ok(rebuilt)
                }
                InstanceReduction::DefaultProtocol => {
                    if let crate::interpreter::IteratorCopy::GetItemReduction {
                        constructor,
                        owner,
                        position,
                    } = crate::interpreter::copy_iterator_object(&obj, true)?
                    {
                        let copied_owner =
                            deep_copy_with_reconstruct(owner, memo, reconstruct, interp)?;
                        let copy = construct_getitem_reduction(constructor, copied_owner, interp)?;
                        if let Some(id) = value_identity(&obj) {
                            memo_insert(memo, id, copy.clone());
                        }
                        apply_getitem_reduction_state(&copy, position, interp)?;
                        Ok(copy)
                    } else if is_exception_class(&class) {
                        // #2360 / #2361: exceptions deep-copy via their `__reduce__`
                        // value — reconstruct `type(*args)` (recursing into args)
                        // and re-apply the deep-copied non-slot state.
                        let obj_id = value_identity(&obj);
                        copy_exception(
                            &rc,
                            interp,
                            |v, interp| deep_copy_with_reconstruct(v, memo, reconstruct, interp),
                            |new_val| {
                                if let Some(id) = obj_id {
                                    memo_insert(memo, id, new_val.clone());
                                }
                            },
                        )
                    } else if crate::builtin_modules::itertools::is_unpicklable_itertools_class(
                        &class,
                    ) {
                        let noun = pyrust_core::error_type_name(&obj);
                        Err(PyError::named(
                            "TypeError",
                            format!("cannot pickle '{noun}' object"),
                        ))
                    } else if let Some(result) = {
                        let mut copy_child =
                            |v: Value, interp: &mut crate::interpreter::Interpreter| {
                                deep_copy_with_reconstruct(v, memo, reconstruct, interp)
                            };
                        let mut put_memo = |id: i64, v: Value| memo_insert(memo, id, v);
                        crate::builtin_modules::itertools::copy_groupby_or_grouper(
                            &rc,
                            interp,
                            &mut copy_child,
                            &mut put_memo,
                            value_identity(&obj),
                        )
                    } {
                        result
                    } else {
                        deep_copy_via_protocol(&rc, &obj, memo, reconstruct, interp)
                    }
                }
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

fn deep_copy(
    obj: Value,
    memo: &Value,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    deep_copy_with_reconstruct(obj, memo, None, interp)
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

/// Default deep-copy path for an instance (#2968).
///
/// Mirrors CPython 3.12's `copy._reconstruct` ordering so an adversarial
/// `__deepcopy__` inspecting `memo.get(id(orig))` never observes a half-built
/// instance lacking its internal backing:
///   - Pre-memo constructor-arg payloads (`Counter`, `set`/`tuple`/`frozenset`
///     subclasses, combinatoric `_cursor` iterators, `defaultdict.default_factory`)
///     deep-copy their constructor inputs *before* `new_val` is memoised.
///   - Post-memo mutable containers (`list`/`dict`/`OrderedDict`/`defaultdict`
///     subclasses and `deque` / `deque` subclasses) install a valid *empty*
///     backing before `memo_insert`, then populate that backing in place so
///     both adversarial observers (`len(seen) == 0`) and self-referential
///     cycles (`s.append(s)`, `d['self'] = d`, `dq.append(dq)`) work cleanly.
///   - Atomic / buffer-backed payloads (`bytearray` and scalar subclasses) copy
///     and install their backing before `memo_insert` (no element recursion).
fn deep_copy_via_protocol(
    rc: &Rc<RefCell<PyInstance>>,
    obj: &Value,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value> {
    let (class, builtin_data, native_slots) = {
        let borrow = rc.borrow();
        (
            Rc::clone(&borrow.class),
            borrow.attrs.builtin_data().cloned(),
            borrow.attrs.slots_snapshot(),
        )
    };
    let is_counter =
        crate::builtin_modules::collections::is_canonical_counter_class_or_subclass(&class);
    let new_rc = Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    }));
    let new_val = Value::py_instance(Rc::clone(&new_rc));

    enum PostMemoBacking {
        None,
        List {
            target: Value,
            items: Vec<Value>,
        },
        Dict {
            target: Value,
            pairs: Vec<(PyKey, Value)>,
        },
        Deque {
            target: Value,
            items: Vec<Value>,
        },
    }

    let mut post_memo = PostMemoBacking::None;

    // 1. Handle `builtin_data` (pre-memo or empty-seeded post-memo).
    if let Some(backing) = builtin_data {
        match backing.kind() {
            ValueKind::List(items) => {
                let snapshot: Vec<Value> = items.iter().cloned().collect();
                drop(items);
                let empty = Value::list(Vec::with_capacity(snapshot.len()));
                new_rc.borrow_mut().attrs.set_builtin_data(empty.clone());
                post_memo = PostMemoBacking::List {
                    target: empty,
                    items: snapshot,
                };
            }
            ValueKind::Dict(d) => {
                let pairs: Vec<(PyKey, Value)> =
                    d.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                drop(d);
                if is_counter {
                    // CPython reduces Counter as `(cls, (dict(self),))`:
                    // deep-copy items before memoising `new_val`.
                    let mut new_map =
                        PyDict::with_capacity_and_hasher(pairs.len(), Default::default());
                    for (k, v) in pairs {
                        let deep_k = {
                            let kv = key_to_value(k);
                            let deep_kv =
                                deep_copy_with_reconstruct(kv, memo, reconstruct, interp)?;
                            interp.value_to_pykey(&deep_kv)?
                        };
                        let deep_v = deep_copy_with_reconstruct(v, memo, reconstruct, interp)?;
                        new_map.insert(deep_k, deep_v);
                    }
                    new_rc
                        .borrow_mut()
                        .attrs
                        .set_builtin_data(Value::dict(new_map));
                } else {
                    let empty = Value::dict(PyDict::with_capacity_and_hasher(
                        pairs.len(),
                        Default::default(),
                    ));
                    new_rc.borrow_mut().attrs.set_builtin_data(empty.clone());
                    post_memo = PostMemoBacking::Dict {
                        target: empty,
                        pairs,
                    };
                }
            }
            ValueKind::Set(_) | ValueKind::Tuple(_) => {
                // CPython passes the deep-copied elements to the set/tuple
                // constructor before memoising the subclass instance.
                let deep_backing =
                    deep_copy_with_reconstruct(backing.clone(), memo, reconstruct, interp)?;
                new_rc.borrow_mut().attrs.set_builtin_data(deep_backing);
            }
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                let deep_backing =
                    deep_copy_with_reconstruct(backing.clone(), memo, reconstruct, interp)?;
                new_rc.borrow_mut().attrs.set_builtin_data(deep_backing);
            }
            _ => {
                // Bytearray (bytes are ints, no child recursion) and scalar
                // immutable backings (`int`, `float`, `str`, `bytes`, …).
                let deep_backing =
                    deep_copy_with_reconstruct(backing.clone(), memo, reconstruct, interp)?;
                new_rc.borrow_mut().attrs.set_builtin_data(deep_backing);
            }
        }
    }

    // 2. Handle native C-style slots (`_items` on deque, `_cursor` on
    //    combinatoric iterators, `default_factory`, `maxlen`, `_buf`, …).
    for (slot_name, slot_value) in native_slots {
        if pyrust_builtins::deque_storage::is_storage(&slot_value) {
            let items = pyrust_builtins::deque_storage::snapshot(&slot_value).unwrap_or_default();
            let empty = pyrust_builtins::deque_storage::deque_storage(Vec::new());
            new_rc
                .borrow_mut()
                .attrs
                .insert_slot(slot_name.as_ref(), empty.clone());
            post_memo = PostMemoBacking::Deque {
                target: empty,
                items,
            };
        } else if let ValueKind::BuiltinObject { ops, state } = slot_value.kind()
            && ops.is_internal_storage()
        {
            // Combinatoric iterator cursor (`_cursor`): deep-copy pool elements
            // BEFORE memoising `new_val`, matching CPython's `(cls, (pool, r))`
            // reduction ordering so `Spy.__deepcopy__` sees `memo[id] is None`
            // and never encounters an uninitialised cursor.
            if let Some(copied_cursor) = ops.copy_storage(state) {
                if let Some(elements) = ops.storage_elements(state) {
                    let mut deep_elements = Vec::with_capacity(elements.len());
                    for element in elements {
                        deep_elements.push(deep_copy_with_reconstruct(
                            element,
                            memo,
                            reconstruct,
                            interp,
                        )?);
                    }
                    if let ValueKind::BuiltinObject {
                        ops: copied_ops,
                        state: copied_state,
                    } = copied_cursor.kind()
                    {
                        copied_ops.set_storage_elements(copied_state, deep_elements);
                    }
                }
                new_rc
                    .borrow_mut()
                    .attrs
                    .insert_slot(slot_name.as_ref(), copied_cursor);
            }
        } else {
            // Non-storage native slot (`default_factory`, `maxlen`, `_buf`, …):
            // deep-copy before memoisation (e.g. `default_factory` is a
            // constructor argument in CPython's `defaultdict.__reduce__`).
            let copied_slot = deep_copy_with_reconstruct(slot_value, memo, reconstruct, interp)?;
            new_rc
                .borrow_mut()
                .attrs
                .insert_slot(slot_name.as_ref(), copied_slot);
        }
    }

    // 3. Memoise the valid (populated or empty-backed) instance *before*
    //    recursing into user `__dict__` state and post-memo container items.
    if let Some(id) = value_identity(obj) {
        memo_insert(memo, id, new_val.clone());
    }

    // 4. Capture state from the original, deep-copy it, then restore.
    let state = capture_state(rc, interp)?;
    let deep_state = deep_copy_with_reconstruct(state, memo, reconstruct, interp)?;
    restore_state(&new_rc, deep_state, interp)?;

    // 5. Populate post-memo mutable container backings in place.
    match post_memo {
        PostMemoBacking::None => {}
        PostMemoBacking::List { target, items } => {
            for item in items {
                let copied = deep_copy_with_reconstruct(item, memo, reconstruct, interp)?;
                target.list_push(copied)?;
            }
        }
        PostMemoBacking::Dict { target, pairs } => {
            for (k, v) in pairs {
                let deep_k = {
                    let kv = key_to_value(k);
                    let deep_kv = deep_copy_with_reconstruct(kv, memo, reconstruct, interp)?;
                    interp.value_to_pykey(&deep_kv)?
                };
                let deep_v = deep_copy_with_reconstruct(v, memo, reconstruct, interp)?;
                let _ = target.dict_insert(deep_k, deep_v);
            }
        }
        PostMemoBacking::Deque { target, items } => {
            if let Some(data) = pyrust_builtins::deque_storage::data(&target) {
                for item in items {
                    let copied = deep_copy_with_reconstruct(item, memo, reconstruct, interp)?;
                    data.borrow_mut().push_back(copied);
                }
            }
        }
    }

    Ok(new_val)
}
