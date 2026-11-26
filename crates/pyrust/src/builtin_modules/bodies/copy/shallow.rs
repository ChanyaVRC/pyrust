// Shallow copy (`copy.copy`) fast paths and default instance protocol.

use std::cell::RefCell;
use std::rc::Rc;

use super::protocol::{
    InstanceReduction, capture_state, copy_exception, resolve_instance_copy_reduction,
    restore_state, unpicklable_error,
};
use super::reconstruct::{
    rebuild_getitem_reduction, rebuild_reduced_iterator, reconstruct_reduction,
};
use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter, is_exception_class};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PySet, Value, ValueKind};

/// Produce a shallow copy of `obj`.  Immutable types are returned as-is;
/// mutable containers get a new top-level allocation with shared elements.
pub(super) fn shallow_copy(obj: Value, interp: &mut Interpreter) -> Result<Value> {
    match obj.kind() {
        // Immutable scalars & immutable sequences — return the same value.
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::Range { .. }
        | ValueKind::Tuple(_) => Ok(obj.clone()),

        // frozenset is immutable — same object.
        ValueKind::BuiltinObject { ops, .. }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
        {
            Ok(obj.clone())
        }

        // list — new list with the same elements (shallow: element Values share Rc).
        ValueKind::List(items) => {
            let new_items: Vec<Value> = items.iter().cloned().collect();
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

        // Opaque built-in storage (bytearray, instance `__dict__` view, deque payload, …).
        ValueKind::BuiltinObject { ops, state } => {
            if pyrust_builtins::mapping_proxy::is_mapping_proxy(&obj) {
                return Err(unpicklable_error("mappingproxy"));
            }
            Ok(ops.copy_storage(state).unwrap_or_else(|| obj.clone()))
        }

        // Built-in iterator objects (#2974).
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
            crate::interpreter::IteratorCopy::Unpicklable(noun) => Err(unpicklable_error(noun)),
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
            shallow_copy_instance(&rc, &obj, interp)
        }

        // Anything else (functions, classes, modules, …) — return as-is.
        _ => Ok(obj.clone()),
    }
}

fn shallow_copy_instance(
    rc: &Rc<RefCell<PyInstance>>,
    obj: &Value,
    interp: &mut Interpreter,
) -> Result<Value> {
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
    match resolve_instance_copy_reduction(obj, &class, false, interp)? {
        InstanceReduction::Reduced(reduction) => {
            reconstruct_reduction(obj, reduction, None, interp)
        }
        InstanceReduction::DefaultProtocol => {
            if let crate::interpreter::IteratorCopy::GetItemReduction {
                constructor,
                owner,
                position,
            } = crate::interpreter::copy_iterator_object(obj, false)?
            {
                rebuild_getitem_reduction(constructor, owner, position, interp)
            } else if is_exception_class(&class) {
                copy_exception(rc, interp, |v, _| Ok(v), |_| {})
            } else if crate::builtin_modules::itertools::is_unpicklable_itertools_class(&class) {
                Err(unpicklable_error(&pyrust_core::error_type_name(obj)))
            } else if let Some(result) = {
                let mut id_copy = |v: Value, _: &mut Interpreter| Ok(v);
                let mut no_memo = |_: i64, _: Value| {};
                crate::builtin_modules::itertools::copy_groupby_or_grouper(
                    rc,
                    interp,
                    &mut id_copy,
                    &mut no_memo,
                    None,
                )
            } {
                result
            } else {
                shallow_copy_via_protocol(rc, interp)
            }
        }
    }
}

/// Default shallow-copy path for an instance: capture state, build a bare
/// instance of the same class, restore state, and copy opaque internal storage.
fn shallow_copy_via_protocol(
    rc: &Rc<RefCell<PyInstance>>,
    interp: &mut Interpreter,
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
/// #2968): detaches `builtin_data` and any internal-storage native slots, while
/// preserving scalar/reference native slots such as `maxlen` and `default_factory`.
fn detach_internal_storage_shallow(
    rc: &Rc<RefCell<PyInstance>>,
    new_rc: &Rc<RefCell<PyInstance>>,
    interp: &mut Interpreter,
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
