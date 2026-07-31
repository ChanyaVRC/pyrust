//! Live mutable proxy for a `PyInstance`'s `attrs` map.
//!
//! Returned by `obj.__dict__` attribute access.  Writes to the proxy (both
//! subscript assignment and dict methods like `update`) propagate immediately
//! to the instance's own `attrs` map, so that patterns like
//!
//! ```python
//! obj.__dict__['x'] = 1   # stores directly in instance
//! obj.__dict__.update({'y': 2})
//! ```
//!
//! behave correctly.  Previously `obj.__dict__` returned a snapshot copy,
//! which silently swallowed writes and broke the data-descriptor `__set__`
//! protocol (issues #1271 / #1272).
//!
//! ## Identity (`vars(obj) is vars(obj)`)
//!
//! CPython guarantees `vars(obj) is vars(obj)` for the same instance.  The
//! typed [`BuiltinTypeOps::identity_payload`] hook below publishes the target
//! `PyInstance` pointer as this proxy type's identity payload.  Core combines
//! it with the concrete ops type in a dedicated namespace, so repeated fresh
//! proxy states for one target agree without colliding with the target object
//! itself.  This avoids caching state in `PyInstance` (which would create a
//! reference cycle).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyDict, PyError, PyInstance, PyKey, Result, Value, ValueKind,
    builtin_ops_is,
};

pub const TYPE_NAME: &str = "instance_dict";

/// Internal state: a live reference to the instance whose visible attrs are
/// proxied. C-style slot values live in `InstanceAttrs`' separate slot storage,
/// so this proxy never needs class-name filtering.
pub struct InstanceDictState {
    pub instance: Rc<RefCell<PyInstance>>,
}

impl InstanceDictState {
    fn visible_len(&self) -> usize {
        self.instance.borrow().attrs.inline_len()
    }

    fn visible_key_at(&self, visible_index: usize) -> Option<Value> {
        let inst = self.instance.borrow();
        inst.attrs
            .get_index(visible_index)
            .map(|(key, _)| Value::string(key.clone()))
    }

    fn visible_keys(&self) -> Vec<Value> {
        self.instance
            .borrow()
            .attrs
            .keys()
            .map(|key| Value::string(key.clone()))
            .collect()
    }
}

pub struct InstanceDictOps;

pub const INSTANCE_DICT_OPS: &InstanceDictOps = &InstanceDictOps;

impl BuiltinTypeOps for InstanceDictOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn canonical_class_tag(&self) -> Option<pyrust_core::CanonicalClassTag> {
        Some(pyrust_core::CanonicalClassTag::Dict)
    }

    fn identity_payload(&self, state: &BuiltinState) -> Option<u64> {
        borrow_state(state).map(|proxy| Rc::as_ptr(&proxy.instance) as u64)
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let s = borrow_state(state).expect("instance_dict state");
        let inst = s.instance.borrow();
        let pairs: Vec<String> = inst
            .attrs
            .iter()
            .map(|(k, v)| format!("{}: {}", Value::string(k.clone()).repr_raw(), v.repr_raw()))
            .collect();
        format!("{{{}}}", pairs.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        let s = match borrow_state(state) {
            Some(s) => s,
            None => return false,
        };
        s.visible_len() != 0
    }

    /// `copy.copy(obj.__dict__)` — CPython copies the mapping, so the result
    /// is a detached `dict`, not a second live view onto the instance.
    /// Returning the proxy itself would let writes to the copy rewrite the
    /// object's attributes (issue #2935).
    fn copy_storage(&self, state: &BuiltinState) -> Option<Value> {
        let s = borrow_state(state)?;
        let inst = s.instance.borrow();
        let mut dict: PyDict =
            PyDict::with_capacity_and_hasher(inst.attrs.inline_len(), Default::default());
        for (key, value) in inst.attrs.iter() {
            dict.insert(PyKey::str_from(key.as_ref()), value.clone());
        }
        Some(Value::dict(dict))
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let s = match borrow_state(state) {
            Some(s) => s,
            None => return false,
        };
        let inst = s.instance.borrow();
        let lhs_pairs: Vec<(&str, &Value)> =
            inst.attrs.iter().map(|(k, v)| (k.as_ref(), v)).collect();
        match other.kind() {
            ValueKind::Dict(rhs) => {
                if lhs_pairs.len() != rhs.len() {
                    return false;
                }
                for (k, v) in &lhs_pairs {
                    match rhs.get(&PyKey::str_from(k)) {
                        Some(other_v) => {
                            if *v != other_v {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
                true
            }
            ValueKind::BuiltinObject {
                ops,
                state: rhs_state,
            } if builtin_ops_is::<InstanceDictOps>(ops) => {
                let rhs_s = match borrow_state(rhs_state) {
                    Some(s) => s,
                    None => return false,
                };
                if Rc::ptr_eq(&s.instance, &rhs_s.instance) {
                    return true;
                }
                let rhs_inst = rhs_s.instance.borrow();
                let rhs_pairs: Vec<(&str, &Value)> = rhs_inst
                    .attrs
                    .iter()
                    .map(|(k, v)| (k.as_ref(), v))
                    .collect();
                if lhs_pairs.len() != rhs_pairs.len() {
                    return false;
                }
                for (k, v) in &lhs_pairs {
                    match rhs_inst.attrs.inline_get(k) {
                        Some(other_v) => {
                            if *v != other_v {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        let s = borrow_state(state)?;
        Some(s.visible_len())
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        // Non-string keys are never present in the instance attrs map (which
        // stores only string keys).  CPython raises KeyError (not TypeError)
        // when a non-string key is absent from a dict — match that behaviour.
        let key_str = match key.kind() {
            ValueKind::Str(k) => k,
            _ => {
                return Err(PyError::key_error(key.clone()));
            }
        };
        let inst = s.instance.borrow();
        inst.attrs
            .inline_get(key_str)
            .cloned()
            .ok_or_else(|| PyError::key_error(Value::string(key_str)))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        let key_str = key_as_str(key)?.to_string();
        s.instance.borrow_mut().attrs.insert_inline(key_str, value);
        Ok(())
    }

    fn delete_item(&self, state: &BuiltinState, key: &Value) -> Result<()> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        // Non-string keys are never present; CPython raises KeyError (not
        // TypeError) for missing keys regardless of key type.
        let key_str = match key.kind() {
            ValueKind::Str(k) => k.to_string(),
            _ => {
                return Err(PyError::key_error(key.clone()));
            }
        };
        let removed = s.instance.borrow_mut().attrs.shift_remove_inline(&key_str);
        if removed.is_none() {
            return Err(PyError::key_error(Value::string(key_str)));
        }
        Ok(())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        let key_str = match item.kind() {
            ValueKind::Str(k) => k.to_string(),
            _ => return Ok(false),
        };
        Ok(s.instance.borrow().attrs.inline_contains_key(&key_str))
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn is_iterator(&self) -> bool {
        false
    }

    fn has_method(&self, name: &str) -> bool {
        matches!(
            name,
            "get"
                | "keys"
                | "values"
                | "items"
                | "pop"
                | "popitem"
                | "setdefault"
                | "update"
                | "copy"
                | "clear"
                | "__contains__"
                | "__len__"
                | "__getitem__"
                | "__setitem__"
                | "__delitem__"
                | "__iter__"
        )
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        match method {
            // Dunder methods delegate to the same write-through paths as the
            // operator forms so `o.__dict__.__setitem__('x', 1)` /
            // `__getitem__` / `__delitem__` / `__contains__` / `__len__`
            // behave identically to `o.__dict__['x'] = 1` etc.  Issue #2163:
            // these were advertised by `has_method` but had no `call_method`
            // arm, so the resolved bound method raised AttributeError on call.
            "__getitem__" => {
                if args.len() != 1 {
                    return Err(takes_exactly_one_err("__getitem__", args.len()));
                }
                drop(s);
                self.get_item(state, &args[0])
            }
            "__setitem__" => {
                if args.len() != 2 {
                    // CPython's mapping `__setitem__` slot wrapper reports
                    // " expected 2 arguments, got N" with a leading space
                    // (a quirk distinct from `__delitem__` / `__len__`).
                    return Err(PyError::named(
                        "TypeError",
                        format!(" expected 2 arguments, got {}", args.len()),
                    ));
                }
                drop(s);
                let mut it = args.into_iter();
                let key = it.next().unwrap();
                let value = it.next().unwrap();
                self.set_item(state, &key, value)?;
                Ok(Value::none())
            }
            "__delitem__" => {
                if args.len() != 1 {
                    return Err(expected_args_err(1, args.len()));
                }
                drop(s);
                self.delete_item(state, &args[0])?;
                Ok(Value::none())
            }
            "__contains__" => {
                if args.len() != 1 {
                    return Err(takes_exactly_one_err("__contains__", args.len()));
                }
                drop(s);
                Ok(Value::bool_(self.contains(state, &args[0])?))
            }
            "__len__" => {
                if !args.is_empty() {
                    return Err(expected_args_err(0, args.len()));
                }
                drop(s);
                Ok(Value::int(self.len(state).unwrap_or(0) as i64))
            }
            "popitem" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("dict.popitem() takes no arguments ({} given)", args.len()),
                    ));
                }
                // dict.popitem() removes and returns the last inserted (key,
                // value) pair (LIFO), raising KeyError on an empty dict.  The
                // attrs key iterator is not double-ended, so scan forward and
                // keep the last key. Resolve it under a shared borrow, then
                // take the mutable borrow to remove it.
                let last_key = {
                    let inst = s.instance.borrow();
                    inst.attrs.keys().last().map(|k| k.to_string())
                };
                let Some(key) = last_key else {
                    return Err(PyError::key_error(Value::string(
                        "popitem(): dictionary is empty",
                    )));
                };
                let value = s
                    .instance
                    .borrow_mut()
                    .attrs
                    .shift_remove_inline(&key)
                    .unwrap();
                Ok(Value::tuple(vec![Value::string(key), value]))
            }
            "get" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::named(
                        "TypeError",
                        format!("get() takes 1 or 2 arguments ({} given)", args.len()),
                    ));
                }
                let key_str = match args[0].kind() {
                    ValueKind::Str(k) => k.to_string(),
                    _ => {
                        return Ok(args.get(1).cloned().unwrap_or(Value::none()));
                    }
                };
                let inst = s.instance.borrow();
                Ok(inst
                    .attrs
                    .inline_get(&key_str)
                    .cloned()
                    .unwrap_or_else(|| args.get(1).cloned().unwrap_or(Value::none())))
            }
            "keys" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("keys() takes no arguments ({} given)", args.len()),
                    ));
                }
                let inst = s.instance.borrow();
                let keys: Vec<Value> = inst
                    .attrs
                    .keys()
                    .map(|k| Value::string(k.clone()))
                    .collect();
                Ok(Value::list(keys))
            }
            "values" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("values() takes no arguments ({} given)", args.len()),
                    ));
                }
                let inst = s.instance.borrow();
                let vals: Vec<Value> = inst.attrs.iter().map(|(_, v)| v.clone()).collect();
                Ok(Value::list(vals))
            }
            "items" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("items() takes no arguments ({} given)", args.len()),
                    ));
                }
                let inst = s.instance.borrow();
                let items: Vec<Value> = inst
                    .attrs
                    .iter()
                    .map(|(k, v)| Value::tuple(vec![Value::string(k.clone()), v.clone()]))
                    .collect();
                Ok(Value::list(items))
            }
            "pop" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::named(
                        "TypeError",
                        format!("pop() takes 1 or 2 arguments ({} given)", args.len()),
                    ));
                }
                let key_str = match args[0].kind() {
                    ValueKind::Str(k) => k.to_string(),
                    _ => {
                        if let Some(default) = args.get(1) {
                            return Ok(default.clone());
                        }
                        return Err(PyError::key_error(args[0].clone()));
                    }
                };
                let removed = s.instance.borrow_mut().attrs.shift_remove_inline(&key_str);
                match removed {
                    Some(v) => Ok(v),
                    None => match args.get(1) {
                        Some(default) => Ok(default.clone()),
                        None => Err(PyError::key_error(Value::string(key_str))),
                    },
                }
            }
            "setdefault" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::named(
                        "TypeError",
                        format!("setdefault() takes 1 or 2 arguments ({} given)", args.len()),
                    ));
                }
                let key_str = match args[0].kind() {
                    ValueKind::Str(k) => k.to_string(),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "instance __dict__ keys must be strings".to_string(),
                        ));
                    }
                };
                let default_val = args.get(1).cloned().unwrap_or(Value::none());
                let mut inst = s.instance.borrow_mut();
                if let Some(existing) = inst.attrs.inline_get(&key_str) {
                    Ok(existing.clone())
                } else {
                    inst.attrs.insert_inline(key_str, default_val.clone());
                    Ok(default_val)
                }
            }
            "update" => {
                // update(dict, **kwargs) — CPython applies the positional
                // mapping first, then the keyword arguments (issue #2163:
                // `update(**kwargs)` previously dropped the keywords because
                // the kwargs map was ignored entirely).
                if args.len() > 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "update() takes at most 1 positional argument ({} given)",
                            args.len()
                        ),
                    ));
                }
                let mut pairs: Vec<(String, Value)> = Vec::new();
                if let Some(other) = args.first() {
                    match other.kind() {
                        ValueKind::Dict(d) => {
                            for (k, v) in d.iter() {
                                match k {
                                    PyKey::Str(ks) => pairs.push((ks.to_string(), v.clone())),
                                    _ => {
                                        return Err(PyError::named(
                                            "TypeError",
                                            "instance __dict__ keys must be strings".to_string(),
                                        ));
                                    }
                                }
                            }
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "update() argument must be a dict, not '{}'",
                                    pyrust_core::builtin_type_name(other),
                                ),
                            ));
                        }
                    }
                }
                for (k, v) in kwargs.iter() {
                    pairs.push((k.clone(), v.clone()));
                }
                if !pairs.is_empty() {
                    let mut inst = s.instance.borrow_mut();
                    for (k, v) in pairs {
                        inst.attrs.insert_inline(k, v);
                    }
                }
                Ok(Value::none())
            }
            "copy" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("copy() takes no arguments ({} given)", args.len()),
                    ));
                }
                let inst = s.instance.borrow();
                let mut dict: PyDict = PyDict::default();
                for (k, v) in inst.attrs.iter() {
                    dict.insert(PyKey::str_from(k), v.clone());
                }
                Ok(Value::dict(dict))
            }
            "clear" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("clear() takes no arguments ({} given)", args.len()),
                    ));
                }
                // Slots are physically separate, so clearing the visible
                // backing is one linear drop with no hide/snapshot/restore pass.
                s.instance.borrow_mut().attrs.clear_inline();
                Ok(Value::none())
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'dict' object has no attribute '{method}'"),
            )),
        }
    }
}

/// Extract the `Rc<RefCell<PyInstance>>` from an `instance_dict` Value, or
/// `None` if the value is not an instance_dict proxy.
pub fn as_instance_rc(value: &Value) -> Option<Rc<RefCell<PyInstance>>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<InstanceDictOps>(ops) {
        return None;
    }
    borrow_state(state).map(|s| Rc::clone(&s.instance))
}

/// Return whether `value` is an `instance_dict` proxy owned by this module.
#[inline]
pub fn is_instance_dict(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::BuiltinObject { ops, .. } if builtin_ops_is::<InstanceDictOps>(ops)
    )
}

/// Number of keys visible through an `instance_dict` proxy.
///
/// Iterator construction records this value and compares it with the live
/// value before every step, matching a real dict iterator's permanent
/// size-change error without putting cursor state on the reusable proxy.
pub fn iter_visible_len(value: &Value) -> Option<usize> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<InstanceDictOps>(ops) {
        return None;
    }
    borrow_state(state).map(|state| state.visible_len())
}

/// Read the key at a live visible insertion-order position.
///
/// This is one direct `Vec::get`: slot mutations use a separate backing and
/// therefore never move the logical dict cursor.
pub fn iter_visible_key_at(value: &Value, visible_index: usize) -> Option<Value> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<InstanceDictOps>(ops) {
        return None;
    }
    borrow_state(state)?.visible_key_at(visible_index)
}

/// Snapshot the currently visible keys for consumers that explicitly
/// materialise an iterable (`list(proxy)`, unpacking, and similar operations).
pub fn iter_visible_keys(value: &Value) -> Option<Vec<Value>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<InstanceDictOps>(ops) {
        return None;
    }
    borrow_state(state).map(|state| state.visible_keys())
}

/// Extract the visible (non-hidden) attrs of an `instance_dict` as a vec of
/// `(PyKey, Value)` pairs, suitable for `DictUpdate` / `**splat`.  Returns
/// `None` if `value` is not an `instance_dict` proxy.
pub fn as_instance_dict_items(value: &Value) -> Option<Vec<(PyKey, Value)>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<InstanceDictOps>(ops) {
        return None;
    }
    let s = borrow_state(state)?;
    let inst = s.instance.borrow();
    Some(
        inst.attrs
            .iter()
            .map(|(k, v)| (PyKey::str_from(k), v.clone()))
            .collect(),
    )
}

/// Return an exception instance's public `__dict__` state.
///
/// Native exception fields live in `InstanceAttrs`' separate slot storage, so
/// this returns only ordinary user attributes. It is the state carried by
/// `BaseException.__reduce__` and `copy`/`deepcopy`; keys keep insertion order.
pub fn exception_dict_state(instance: &Rc<RefCell<PyInstance>>) -> Vec<(String, Value)> {
    let inst = instance.borrow();
    inst.attrs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// Construct an `instance_dict` proxy wrapping a live `PyInstance` reference.
///
/// Each call returns a fresh `BuiltinObject` value whose backing `Rc` is
/// distinct.  The ops table's typed identity hook makes each state expose the
/// shared target instance identity to core.
pub fn instance_dict(instance: Rc<RefCell<PyInstance>>) -> Value {
    let state: Box<dyn Any> = Box::new(InstanceDictState { instance });
    Value::builtin_object(INSTANCE_DICT_OPS, state)
}

fn borrow_state(state: &BuiltinState) -> Option<std::cell::Ref<'_, InstanceDictState>> {
    let borrow = state.borrow();
    // `Ref::filter_map` is stable since 1.63.
    std::cell::Ref::filter_map(borrow, |any| any.downcast_ref::<InstanceDictState>()).ok()
}

/// CPython's `dict.__getitem__` / `__contains__` C wrappers report
/// "dict.<method>() takes exactly one argument (N given)".
fn takes_exactly_one_err(method: &str, given: usize) -> PyError {
    PyError::named(
        "TypeError",
        format!("dict.{method}() takes exactly one argument ({given} given)"),
    )
}

/// CPython's `__delitem__` / `__len__` slot wrappers report
/// "expected N argument(s), got M" (no method name, no leading space —
/// `__setitem__` has its own leading-space quirk handled inline).
fn expected_args_err(expected: usize, given: usize) -> PyError {
    let noun = if expected == 1 {
        "argument"
    } else {
        "arguments"
    };
    PyError::named(
        "TypeError",
        format!("expected {expected} {noun}, got {given}"),
    )
}

fn key_as_str(key: &Value) -> Result<&str> {
    match key.kind() {
        ValueKind::Str(s) => Ok(s),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "attribute name must be string, not '{}'",
                pyrust_core::builtin_type_name(key),
            ),
        )),
    }
}
