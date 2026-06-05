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
//! CPython guarantees `vars(obj) is vars(obj)` for the same instance.  pyrust
//! implements this in `values_are_identical` (helpers.rs): when both operands
//! are `instance_dict` proxies, identity is True iff both reference the same
//! underlying `PyInstance` (checked via `Rc::ptr_eq`).  This avoids caching
//! state in `PyInstance` (which would create a reference cycle) while still
//! satisfying the CPython parity test.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyDict, PyError, PyInstance, PyKey, Result, Value, ValueKind,
};

pub const TYPE_NAME: &str = "instance_dict";

/// Internal state: a live reference to the instance whose attrs are proxied.
/// Also holds exception-class filtering state, computed once at construction.
pub struct InstanceDictState {
    pub instance: Rc<RefCell<PyInstance>>,
    /// When `true`, attrs that are exception C-level slots are hidden from
    /// public iteration/repr (matching CPython's `__dict__` on BaseException).
    pub is_exception: bool,
    /// When `true`, the class (or an ancestor) declares `__slots__`, so the
    /// `__dict__` proxy must hide slot-member keys (issue #2076).  Cached once
    /// so the common no-slots instance pays no per-key MRO walk.
    has_slots: bool,
    /// Cached key snapshot for iteration (lazily materialised on first
    /// `iter_next` call, reset by mutating methods).  Uses interior
    /// mutability so `iter_next` can advance the cursor through `&self`.
    iter_keys: RefCell<Option<Vec<String>>>,
    iter_pos: RefCell<usize>,
}

impl InstanceDictState {
    /// Returns `true` when `name` is a C-level exception slot that must be
    /// hidden from `__dict__` for this instance.
    fn is_hidden(&self, name: &str) -> bool {
        // Issue #2076: a `__slots__` member is stored in the instance's attrs
        // map but is slot storage, not a `__dict__` entry, so it is hidden from
        // the `__dict__` proxy (relevant only for `__slots__ = (..., '__dict__')`,
        // where both a dict and member slots coexist).  Gated on `has_slots` so
        // the common no-slots instance skips the MRO walk.
        if self.has_slots {
            let class = Rc::clone(&self.instance.borrow().class);
            if is_slot_member(name, &class) {
                return true;
            }
        }
        if !self.is_exception {
            return false;
        }
        if is_exc_hidden_slot(name) {
            return true;
        }
        let class = Rc::clone(&self.instance.borrow().class);
        is_exc_class_slot(name, &class)
    }
}

pub struct InstanceDictOps;

pub const INSTANCE_DICT_OPS: &InstanceDictOps = &InstanceDictOps;

impl BuiltinTypeOps for InstanceDictOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let s = borrow_state(state).expect("instance_dict state");
        let inst = s.instance.borrow();
        let pairs: Vec<String> = inst
            .attrs
            .iter()
            .filter(|(k, _)| !s.is_hidden(k))
            .map(|(k, v)| format!("{}: {}", Value::string(k.clone()).repr(), v.repr()))
            .collect();
        format!("{{{}}}", pairs.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        let s = match borrow_state(state) {
            Some(s) => s,
            None => return false,
        };
        let inst = s.instance.borrow();
        inst.attrs.keys().any(|k| !s.is_hidden(k))
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let s = match borrow_state(state) {
            Some(s) => s,
            None => return false,
        };
        let inst = s.instance.borrow();
        let lhs_pairs: Vec<(&str, &Value)> = inst
            .attrs
            .iter()
            .filter(|(k, _)| !s.is_hidden(k))
            .map(|(k, v)| (k.as_ref(), v))
            .collect();
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
            } if ops.type_name() == TYPE_NAME => {
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
                    .filter(|(k, _)| !rhs_s.is_hidden(k))
                    .map(|(k, v)| (k.as_ref(), v))
                    .collect();
                if lhs_pairs.len() != rhs_pairs.len() {
                    return false;
                }
                for (k, v) in &lhs_pairs {
                    match rhs_inst.attrs.get(k) {
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
        let inst = s.instance.borrow();
        Some(inst.attrs.keys().filter(|k| !s.is_hidden(k)).count())
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
        // Hidden exception C-level slots must not be accessible via subscript.
        if s.is_hidden(key_str) {
            return Err(PyError::key_error(Value::string(key_str)));
        }
        let inst = s.instance.borrow();
        inst.attrs
            .get(key_str)
            .cloned()
            .ok_or_else(|| PyError::key_error(Value::string(key_str)))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        let key_str = key_as_str(key)?.to_string();
        s.instance.borrow_mut().attrs.insert(key_str, value);
        // Invalidate the iteration snapshot so a mutation during iteration
        // restarts cleanly (matches CPython's RuntimeError on dict-size-change
        // in the middle of iteration — we simply reset rather than error).
        s.iter_keys.borrow_mut().take();
        *s.iter_pos.borrow_mut() = 0;
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
        let removed = s.instance.borrow_mut().attrs.shift_remove(&key_str);
        if removed.is_none() {
            return Err(PyError::key_error(Value::string(key_str)));
        }
        s.iter_keys.borrow_mut().take();
        *s.iter_pos.borrow_mut() = 0;
        Ok(())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        let key_str = match item.kind() {
            ValueKind::Str(k) => k.to_string(),
            _ => return Ok(false),
        };
        // Hidden exception C-level slots must not appear in __dict__.
        if s.is_hidden(&key_str) {
            return Ok(false);
        }
        Ok(s.instance.borrow().attrs.contains_key(&key_str))
    }

    fn is_iterable(&self) -> bool {
        true
    }

    /// Iteration over an instance dict yields its keys (strings), like `dict`.
    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        let s = borrow_state(state)
            .ok_or_else(|| PyError::Runtime("internal: bad instance_dict state".to_string()))?;
        // Materialise key snapshot lazily on first call.
        {
            let mut iter_keys = s.iter_keys.borrow_mut();
            if iter_keys.is_none() {
                let inst = s.instance.borrow();
                *iter_keys = Some(
                    inst.attrs
                        .keys()
                        .filter(|k| !s.is_hidden(k))
                        .map(|k| k.to_string())
                        .collect(),
                );
            }
        }
        let iter_keys_ref = s.iter_keys.borrow();
        let keys = iter_keys_ref.as_ref().unwrap();
        let mut pos = s.iter_pos.borrow_mut();
        if *pos < keys.len() {
            let k = keys[*pos].clone();
            *pos += 1;
            Ok(Some(Value::string(k)))
        } else {
            Ok(None)
        }
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
                // keep the last visible key.  `is_hidden` borrows the instance
                // (slotted/exception classes), so resolve the key under a
                // shared borrow first, then take the mutable borrow to remove
                // it — borrowing mutably across `is_hidden` would panic.
                let last_key = {
                    let inst = s.instance.borrow();
                    inst.attrs
                        .keys()
                        .filter(|k| !s.is_hidden(k))
                        .last()
                        .map(|k| k.to_string())
                };
                let Some(key) = last_key else {
                    return Err(PyError::key_error(Value::string(
                        "popitem(): dictionary is empty",
                    )));
                };
                let value = s.instance.borrow_mut().attrs.shift_remove(&key).unwrap();
                s.iter_keys.borrow_mut().take();
                *s.iter_pos.borrow_mut() = 0;
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
                // Hidden exception C-level slots are not visible via get().
                if s.is_hidden(&key_str) {
                    return Ok(args.get(1).cloned().unwrap_or(Value::none()));
                }
                let inst = s.instance.borrow();
                Ok(inst
                    .attrs
                    .get(&key_str)
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
                    .filter(|k| !s.is_hidden(k))
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
                let vals: Vec<Value> = inst
                    .attrs
                    .iter()
                    .filter(|(k, _)| !s.is_hidden(k))
                    .map(|(_, v)| v.clone())
                    .collect();
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
                    .filter(|(k, _)| !s.is_hidden(k))
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
                let removed = s.instance.borrow_mut().attrs.shift_remove(&key_str);
                s.iter_keys.borrow_mut().take();
                *s.iter_pos.borrow_mut() = 0;
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
                if let Some(existing) = inst.attrs.get(&key_str) {
                    Ok(existing.clone())
                } else {
                    inst.attrs.insert(key_str, default_val.clone());
                    drop(inst);
                    s.iter_keys.borrow_mut().take();
                    *s.iter_pos.borrow_mut() = 0;
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
                        inst.attrs.insert(k, v);
                    }
                    drop(inst);
                    s.iter_keys.borrow_mut().take();
                    *s.iter_pos.borrow_mut() = 0;
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
                for (k, v) in inst.attrs.iter().filter(|(k, _)| !s.is_hidden(k)) {
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
                // Only remove visible `__dict__` entries: hidden slot members
                // (`__slots__ = (..., '__dict__')`) and exception C-level slots
                // are stored in `attrs` but are not part of the `__dict__`
                // proxy, so `dict.clear()` must leave them intact (CPython keeps
                // the slot value alive after `o.__dict__.clear()`).  The common
                // no-slots, non-exception instance has no hidden keys, so wipe
                // the whole map in one shot.  Otherwise resolve the visible keys
                // under a shared borrow first (`is_hidden` re-borrows the
                // instance), then take the mutable borrow to remove only them.
                if !s.has_slots && !s.is_exception {
                    s.instance.borrow_mut().attrs.clear();
                } else {
                    let visible: Vec<String> = {
                        let inst = s.instance.borrow();
                        inst.attrs
                            .keys()
                            .filter(|k| !s.is_hidden(k))
                            .map(|k| k.to_string())
                            .collect()
                    };
                    let mut inst = s.instance.borrow_mut();
                    for k in &visible {
                        inst.attrs.shift_remove(k);
                    }
                }
                s.iter_keys.borrow_mut().take();
                *s.iter_pos.borrow_mut() = 0;
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
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    borrow_state(state).map(|s| Rc::clone(&s.instance))
}

/// Extract the visible (non-hidden) attrs of an `instance_dict` as a vec of
/// `(PyKey, Value)` pairs, suitable for `DictUpdate` / `**splat`.  Returns
/// `None` if `value` is not an `instance_dict` proxy.
pub fn as_instance_dict_items(value: &Value) -> Option<Vec<(PyKey, Value)>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    let s = borrow_state(state)?;
    let inst = s.instance.borrow();
    Some(
        inst.attrs
            .iter()
            .filter(|(k, _)| !s.is_hidden(k))
            .map(|(k, v)| (PyKey::str_from(k), v.clone()))
            .collect(),
    )
}

/// Construct an `instance_dict` proxy wrapping a live `PyInstance` reference.
///
/// Each call returns a fresh `BuiltinObject` value whose backing `Rc` is
/// distinct.  CPython identity (`vars(obj) is vars(obj)`) is implemented
/// separately in `values_are_identical` by comparing the `instance` `Rc`
/// pointers of two `instance_dict` proxies.
pub fn instance_dict(instance: Rc<RefCell<PyInstance>>, is_exception: bool) -> Value {
    let has_slots = class_chain_has_slots(&instance.borrow().class);
    let state: Box<dyn Any> = Box::new(InstanceDictState {
        instance,
        is_exception,
        has_slots,
        iter_keys: RefCell::new(None),
        iter_pos: RefCell::new(0),
    });
    Value::builtin_object(INSTANCE_DICT_OPS, state)
}

/// Walk the class base chain and return `true` if any class declares
/// `__slots__` (issue #2076).  Cached on the proxy so per-key `is_hidden`
/// checks only run the slot-member walk for slotted classes.
fn class_chain_has_slots(class: &Rc<RefCell<pyrust_core::PyClass>>) -> bool {
    let (has, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.slots.is_some(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    has || base.is_some_and(|b| class_chain_has_slots(&b))
        || extra_bases.iter().any(class_chain_has_slots)
}

/// Return `true` if both `BuiltinState` values are `instance_dict` proxies for
/// the same underlying `PyInstance` (by `Rc` pointer equality).
///
/// Used by `values_are_identical` to implement `vars(a) is vars(a)` → `True`
/// without caching the proxy inside the instance.
pub fn same_instance(a: &BuiltinState, b: &BuiltinState) -> bool {
    let a_s = match borrow_state(a) {
        Some(s) => s,
        None => return false,
    };
    let b_s = match borrow_state(b) {
        Some(s) => s,
        None => return false,
    };
    Rc::ptr_eq(&a_s.instance, &b_s.instance)
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

/// Returns `true` when `name` is a C-level slot on the given exception class
/// that CPython 3.12 hides from `__dict__`.
///
/// Mirrors the full logic of `is_exc_c_slot` in the interpreter's `helpers.rs`
/// but operates solely on `PyInstance` / `PyClass` types from `pyrust-core`
/// (no interpreter dependency).
fn is_exc_hidden_slot(name: &str) -> bool {
    // Universal BaseException C-level slots — always hidden for any exception.
    matches!(
        name,
        "args" | "__cause__" | "__context__" | "__suppress_context__" | "__traceback__"
    )
}

/// Returns `true` when `name` is a class-specific C-level slot that must be
/// hidden for instances of `class_name` or its subclasses.
fn is_exc_class_slot(name: &str, class: &Rc<RefCell<pyrust_core::PyClass>>) -> bool {
    // StopIteration.value
    if name == "value" && class_chain_has(class, "StopIteration") {
        return true;
    }
    // SystemExit.code
    if name == "code" && class_chain_has(class, "SystemExit") {
        return true;
    }
    // SyntaxError structured attrs
    if matches!(
        name,
        "msg"
            | "filename"
            | "lineno"
            | "offset"
            | "text"
            | "end_lineno"
            | "end_offset"
            | "print_file_and_line"
    ) && class_chain_has(class, "SyntaxError")
    {
        return true;
    }
    // OSError attrs
    if matches!(name, "errno" | "strerror" | "filename" | "filename2")
        && class_chain_has(class, "OSError")
    {
        return true;
    }
    // ImportError: name/path
    if matches!(name, "name" | "path") && class_chain_has(class, "ImportError") {
        return true;
    }
    false
}

/// Walk the class base chain and return `true` if `name` resolves to a
/// `__slots__` member_descriptor (issue #2076).  Such names are stored in the
/// instance's `attrs` map but represent slot storage, not the per-instance
/// `__dict__`, so they must be hidden from the `__dict__` proxy when the class
/// also has a `__dict__` slot (`__slots__ = ('q', '__dict__')`).
fn is_slot_member(name: &str, class: &Rc<RefCell<pyrust_core::PyClass>>) -> bool {
    let (attr, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.attrs.get(name).cloned(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if let Some(v) = attr {
        return crate::member_descriptor::as_member_descriptor(&v).is_some();
    }
    if let Some(b) = base
        && is_slot_member(name, &b)
    {
        return true;
    }
    extra_bases.iter().any(|b| is_slot_member(name, b))
}

/// Walk the class base chain and return `true` if any class has `target_name`.
fn class_chain_has(class: &Rc<RefCell<pyrust_core::PyClass>>, target_name: &str) -> bool {
    let (name, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.name.clone(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if name == target_name {
        return true;
    }
    if let Some(b) = base
        && class_chain_has(&b, target_name)
    {
        return true;
    }
    extra_bases.iter().any(|b| class_chain_has(b, target_name))
}
