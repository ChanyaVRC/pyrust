// Deep copy (`copy.deepcopy`) traversal for built-in containers, opaque
// storage, iterators, and instances.

use std::cell::RefCell;
use std::rc::Rc;

use super::deep_protocol::deep_copy_via_protocol;
use super::memo::{
    DeepCopyDepthGuard, deepcopy_kept_identity, memo_get, memo_insert, memo_insert_obj,
    value_identity,
};
use super::protocol::{
    InstanceReduction, copy_exception, resolve_instance_copy_reduction, unpicklable_error,
};
use super::reconstruct::{
    apply_getitem_reduction_state, construct_getitem_reduction, rebuild_reduced_iterator,
    reconstruct_reduction_deep,
};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, is_exception_class, key_to_value};
use crate::value::{PyDict, PyInstance, PyKey, PySet, Value, ValueKind};

pub(super) fn deep_copy(obj: Value, memo: &Value, interp: &mut Interpreter) -> Result<Value> {
    deep_copy_with_reconstruct(obj, memo, None, interp)
}

pub(super) fn deep_copy_key(
    key: PyKey,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut Interpreter,
) -> Result<PyKey> {
    let kv = key_to_value(key);
    let deep_kv = deep_copy_with_reconstruct(kv, memo, reconstruct, interp)?;
    interp.value_to_pykey(&deep_kv)
}

fn deep_copy_set_keys(
    keys: Vec<PyKey>,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut Interpreter,
) -> Result<PySet> {
    let mut new_set: PySet = PySet::with_capacity_and_hasher(keys.len(), Default::default());
    for k in keys {
        let new_k = deep_copy_key(k, memo, reconstruct, interp)?;
        new_set.insert(new_k);
    }
    Ok(new_set)
}

pub(super) fn deep_copy_storage_elements(
    ops: &'static dyn pyrust_core::BuiltinTypeOps,
    state: &pyrust_core::BuiltinState,
    copied: &Value,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut Interpreter,
) -> Result<bool> {
    let Some(elements) = ops.storage_elements(state) else {
        return Ok(false);
    };
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
    Ok(true)
}

/// Produce a deep copy of `obj`.  Immutable types are returned as-is.
/// Mutable containers are recursively copied.  The `memo` dict (keyed by
/// `id`) terminates cycles and preserves shared references; the copy of a
/// container/instance is inserted into the memo *before* its children are
/// recursed into.
pub(super) fn deep_copy_with_reconstruct(
    obj: Value,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut Interpreter,
) -> Result<Value> {
    let _depth_guard = DeepCopyDepthGuard::enter(interp)?;
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

        // frozenset — deep-copy elements, rebuild frozenset, memoise.
        ValueKind::BuiltinObject { ops, .. }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
        {
            let items_rc = pyrust_builtins::frozenset::as_items(&obj)
                .expect("frozenset arm: as_items must succeed");
            let keys: Vec<PyKey> = items_rc.iter().cloned().collect();
            drop(items_rc);
            let new_set = deep_copy_set_keys(keys, memo, reconstruct, interp)?;
            let result = pyrust_builtins::frozenset::frozenset(new_set);
            memo_insert_obj(memo, &obj, &result);
            Ok(result)
        }

        // tuple — deep-copy each element; honour cycles and preserve identity
        // when every element was identity-unchanged.
        ValueKind::Tuple(items) => {
            let items_vec: Vec<Value> = items.to_vec();
            let mut new_items = Vec::with_capacity(items_vec.len());
            let mut all_same = true;
            for item in &items_vec {
                let copied = deep_copy_with_reconstruct(item.clone(), memo, reconstruct, interp)?;
                if !deepcopy_kept_identity(item, &copied) {
                    all_same = false;
                }
                new_items.push(copied);
            }
            if let Some(id) = value_identity(&obj)
                && let Some(existing) = memo_get(memo, id)
            {
                return Ok(existing);
            }
            if all_same {
                return Ok(obj.clone());
            }
            let result = Value::tuple(new_items);
            memo_insert_obj(memo, &obj, &result);
            Ok(result)
        }

        // list — insert empty list into memo before recursing.
        ValueKind::List(items) => {
            let items_vec: Vec<Value> = items.iter().cloned().collect();
            drop(items);
            let new_list = Value::list(Vec::with_capacity(items_vec.len()));
            memo_insert_obj(memo, &obj, &new_list);
            for item in items_vec {
                let copied = deep_copy_with_reconstruct(item, memo, reconstruct, interp)?;
                new_list.list_push(copied)?;
            }
            Ok(new_list)
        }

        // dict — insert empty dict into memo before recursing.
        ValueKind::Dict(d) => {
            let pairs: Vec<(PyKey, Value)> =
                d.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            drop(d);
            let new_dict = Value::dict(PyDict::with_capacity_and_hasher(
                pairs.len(),
                Default::default(),
            ));
            memo_insert_obj(memo, &obj, &new_dict);
            for (k, v) in pairs {
                let deep_k = deep_copy_key(k, memo, reconstruct, interp)?;
                let deep_v = deep_copy_with_reconstruct(v, memo, reconstruct, interp)?;
                let _ = new_dict.dict_insert(deep_k, deep_v);
            }
            Ok(new_dict)
        }

        // set — memoise empty set before recursing into hashable elements.
        ValueKind::Set(items) => {
            let keys: Vec<PyKey> = items.iter().cloned().collect();
            drop(items);
            let new_set_val = Value::set(PySet::default());
            memo_insert_obj(memo, &obj, &new_set_val);
            let new_set = deep_copy_set_keys(keys, memo, reconstruct, interp)?;
            new_set_val.set_with_mut(|s| *s = new_set).ok_or_else(|| {
                PyError::named("TypeError", "deepcopy: set rebuild failed".to_string())
            })?;
            Ok(new_set_val)
        }

        // Opaque built-in storage.
        ValueKind::BuiltinObject { ops, state } => {
            if pyrust_builtins::mapping_proxy::is_mapping_proxy(&obj) {
                return Err(unpicklable_error("mappingproxy"));
            }
            let Some(copied) = ops.copy_storage(state) else {
                return Ok(obj.clone());
            };
            memo_insert_obj(memo, &obj, &copied);
            if !deep_copy_storage_elements(ops, state, &copied, memo, reconstruct, interp)?
                && let Some(pairs) = copied.dict_with(|d| {
                    d.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>()
                })
            {
                for (key, value) in pairs {
                    let deep_value = deep_copy_with_reconstruct(value, memo, reconstruct, interp)?;
                    let _ = copied.dict_insert(key, deep_value);
                }
            }
            Ok(copied)
        }

        // Built-in iterator objects (#2974).
        ValueKind::Generator(_) => deep_copy_generator(&obj, memo, reconstruct, interp),

        // PyInstance — CPython 3.12 copy.deepcopy precedence.
        ValueKind::PyInstance(rc) => {
            let rc = Rc::clone(rc);
            deep_copy_instance(&rc, &obj, memo, reconstruct, interp)
        }

        // Anything else — return as-is.
        _ => Ok(obj.clone()),
    }
}

fn deep_copy_generator(
    obj: &Value,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut Interpreter,
) -> Result<Value> {
    let rebuilt = match crate::interpreter::copy_iterator_object(obj, true)? {
        crate::interpreter::IteratorCopy::Rebuilt(copy) => copy,
        crate::interpreter::IteratorCopy::BytearrayReduction { carrier, position } => {
            let copied_carrier = deep_copy_with_reconstruct(carrier, memo, reconstruct, interp)?;
            let copy = rebuild_reduced_iterator(copied_carrier, position, interp)?;
            memo_insert_obj(memo, obj, &copy);
            return Ok(copy);
        }
        crate::interpreter::IteratorCopy::GetItemReduction {
            constructor,
            owner,
            position,
        } => {
            let copied_owner = deep_copy_with_reconstruct(owner, memo, reconstruct, interp)?;
            let copy = construct_getitem_reduction(constructor, copied_owner, interp)?;
            memo_insert_obj(memo, obj, &copy);
            apply_getitem_reduction_state(&copy, position, interp)?;
            return Ok(copy);
        }
        crate::interpreter::IteratorCopy::Unpicklable(noun) => {
            return Err(unpicklable_error(noun));
        }
        crate::interpreter::IteratorCopy::Unowned => {
            let mut copy_child =
                |v: Value| deep_copy_with_reconstruct(v, memo, reconstruct, interp);
            let mut get_memo = |id: i64| memo_get(memo, id);
            let mut put_memo = |id: i64, v: Value| memo_insert(memo, id, v);
            return Ok(crate::interpreter::copy_provider_iterator(
                obj,
                true,
                &mut copy_child,
                &mut get_memo,
                &mut put_memo,
                value_identity(obj),
            )?
            .unwrap_or_else(|| obj.clone()));
        }
    };
    memo_insert_obj(memo, obj, &rebuilt);
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

fn deep_copy_instance(
    rc: &Rc<RefCell<PyInstance>>,
    obj: &Value,
    memo: &Value,
    reconstruct: Option<&Value>,
    interp: &mut Interpreter,
) -> Result<Value> {
    let class = Rc::clone(&rc.borrow().class);
    match interp.get_attr(obj, "__deepcopy__") {
        Ok(copier) if !copier.is_none() => {
            let result = interp.call_function_expanded(
                copier,
                &[ExpandedCallArg {
                    name: None,
                    value: memo.clone(),
                }],
            )?;
            if !deepcopy_kept_identity(obj, &result) {
                memo_insert_obj(memo, obj, &result);
            }
            return Ok(result);
        }
        Ok(_) => {}
        Err(error) if error.class_name_is("AttributeError") => {}
        Err(error) => return Err(error),
    }
    match resolve_instance_copy_reduction(obj, &class, true, interp)? {
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
                reconstruct_reduction_deep(obj, reduction, memo, interp)?
            };
            if !deepcopy_kept_identity(obj, &rebuilt) {
                memo_insert_obj(memo, obj, &rebuilt);
            }
            Ok(rebuilt)
        }
        InstanceReduction::DefaultProtocol => {
            if let crate::interpreter::IteratorCopy::GetItemReduction {
                constructor,
                owner,
                position,
            } = crate::interpreter::copy_iterator_object(obj, true)?
            {
                let copied_owner = deep_copy_with_reconstruct(owner, memo, reconstruct, interp)?;
                let copy = construct_getitem_reduction(constructor, copied_owner, interp)?;
                memo_insert_obj(memo, obj, &copy);
                apply_getitem_reduction_state(&copy, position, interp)?;
                Ok(copy)
            } else if is_exception_class(&class) {
                let obj_id = value_identity(obj);
                copy_exception(
                    rc,
                    interp,
                    |v, interp| deep_copy_with_reconstruct(v, memo, reconstruct, interp),
                    |new_val| {
                        if let Some(id) = obj_id {
                            memo_insert(memo, id, new_val.clone());
                        }
                    },
                )
            } else if crate::builtin_modules::itertools::is_unpicklable_itertools_class(&class) {
                Err(unpicklable_error(&pyrust_core::error_type_name(obj)))
            } else if let Some(result) = {
                let mut copy_child = |v: Value, interp: &mut Interpreter| {
                    deep_copy_with_reconstruct(v, memo, reconstruct, interp)
                };
                let mut put_memo = |id: i64, v: Value| memo_insert(memo, id, v);
                crate::builtin_modules::itertools::copy_groupby_or_grouper(
                    rc,
                    interp,
                    &mut copy_child,
                    &mut put_memo,
                    value_identity(obj),
                )
            } {
                result
            } else {
                deep_copy_via_protocol(rc, obj, memo, reconstruct, interp)
            }
        }
    }
}
