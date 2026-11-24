// Copy protocol resolution (`__reduce_ex__` / `__reduce__`), instance state
// capture/restore (`__getstate__` / `__setstate__`), and exception copying.

use std::cell::RefCell;
use std::rc::Rc;

use super::memo::current_copy_error_class_value;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, invoke_class_method, is_exception_class, lookup_class_attr,
};
use crate::value::{PyClass, PyInstance, PyKey, Value, ValueKind};

pub(super) enum InstanceReduction {
    /// Inherited canonical reducer with no user override: keep the native
    /// default protocol (exception reconstruction or `__getstate__`/`__setstate__`
    /// plus `#2945` internal built-in storage detachment).
    DefaultProtocol,
    /// A custom `__reduce_ex__` or `__reduce__` hook returned a reduction value
    /// to pass through `_reconstruct`.
    Reduced(Value),
}

pub(super) fn unpicklable_error(noun: &str) -> PyError {
    PyError::named("TypeError", format!("cannot pickle '{noun}' object"))
}

pub(super) fn raise_uncopyable_error(
    class: &Rc<RefCell<PyClass>>,
    is_deep: bool,
    interp: &mut Interpreter,
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
pub(super) fn resolve_instance_copy_reduction(
    obj: &Value,
    class: &Rc<RefCell<PyClass>>,
    is_deep: bool,
    interp: &mut Interpreter,
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

/// Capture the copyable state of the instance behind `rc`.  If the class
/// defines `__getstate__`, call it; otherwise the default state is the
/// instance `__dict__` as a Python `dict` (or `None` when empty, matching
/// CPython 3.12's `object.__getstate__`).
pub(super) fn capture_state(
    rc: &Rc<RefCell<PyInstance>>,
    interp: &mut Interpreter,
) -> Result<Value> {
    let class = Rc::clone(&rc.borrow().class);
    if let Some(method) = lookup_class_attr(&class, "__getstate__") {
        return invoke_class_method(interp, method, Value::py_instance(Rc::clone(rc)), &[]);
    }
    Ok(super::default_instance_state(rc))
}

/// Restore captured `state` onto the bare instance `rc`.  If the class defines
/// `__setstate__`, call it; otherwise default to `__dict__.update(state)` (and
/// `setattr` for slot state when `state` is a `(dict_state, slot_state)` pair).
pub(super) fn restore_state(
    rc: &Rc<RefCell<PyInstance>>,
    state: Value,
    interp: &mut Interpreter,
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
    interp: &mut Interpreter,
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

/// Reconstruct a copy of an exception instance via `type(*args)` + non-slot
/// `__dict__` state (#2360 / #2361).
pub(super) fn copy_exception(
    rc: &Rc<RefCell<PyInstance>>,
    interp: &mut Interpreter,
    mut copy_val: impl FnMut(Value, &mut Interpreter) -> Result<Value>,
    on_constructed: impl FnOnce(&Value),
) -> Result<Value> {
    let cls = crate::interpreter::value_class(&Value::py_instance(Rc::clone(rc)));
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
    let new_val = interp.call_function_expanded(cls, &ctor_args)?;
    on_constructed(&new_val);
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
