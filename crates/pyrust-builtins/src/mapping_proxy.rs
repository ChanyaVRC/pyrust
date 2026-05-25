//! `mappingproxy` — a read-only view of a `PyClass`'s attribute dict.
//!
//! Returned by `vars(SomeClass)`.  Delegates reads to the live class attrs
//! (`IndexMap<String, Value>`) and raises `TypeError` on any mutation attempt,
//! matching CPython 3.12's `types.MappingProxyType` behaviour.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyClass, PyError, PyKey, Result, Value, ValueKind,
};

pub const TYPE_NAME: &str = "mappingproxy";

/// Internal state: a live reference to the class whose attrs are being proxied.
pub struct MappingProxyState {
    pub class: Rc<RefCell<PyClass>>,
}

pub struct MappingProxyOps;

pub const MAPPING_PROXY_OPS: &MappingProxyOps = &MappingProxyOps;

impl BuiltinTypeOps for MappingProxyOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let cls = borrow_class(state).expect("mappingproxy state");
        let class = cls.borrow();
        let inner: Vec<String> = class
            .attrs
            .iter()
            .map(|(k, v)| format!("{}: {}", Value::string(k.clone()).repr(), v.repr()))
            .collect();
        format!("mappingproxy({{{}}})", inner.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        borrow_class(state)
            .map(|rc| !rc.borrow().attrs.is_empty())
            .unwrap_or(false)
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let cls = match borrow_class(state) {
            Some(c) => c,
            None => return false,
        };
        // mappingproxy compares equal to a dict with the same content.
        match other.kind() {
            ValueKind::Dict(rhs) => {
                let class = cls.borrow();
                if class.attrs.len() != rhs.len() {
                    return false;
                }
                for (k, v) in class.attrs.iter() {
                    match rhs.get(&PyKey::str_from(k)) {
                        Some(other_v) => {
                            if v != other_v {
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
                let rhs_cls = match borrow_class(rhs_state) {
                    Some(c) => c,
                    None => return false,
                };
                Rc::ptr_eq(&cls, &rhs_cls) || {
                    let lhs = cls.borrow();
                    let rhs = rhs_cls.borrow();
                    lhs.attrs == rhs.attrs
                }
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        borrow_class(state).map(|rc| rc.borrow().attrs.len())
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let cls = borrow_class(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        let key_str = match key.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named("KeyError", key.repr()));
            }
        };
        let class = cls.borrow();
        class
            .attrs
            .get(&key_str)
            .cloned()
            .ok_or_else(|| PyError::named("KeyError", key.repr()))
    }

    // `set_item` raises TypeError — the default BuiltinTypeOps impl already
    // produces "'mappingproxy' object does not support item assignment".

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let cls = borrow_class(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        let key_str = match item.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Ok(false),
        };
        Ok(cls.borrow().attrs.contains_key(&key_str))
    }

    fn has_method(&self, name: &str) -> bool {
        matches!(name, "keys" | "values" | "items" | "get" | "copy")
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let cls = borrow_class(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        match method {
            "keys" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("keys() takes no arguments ({} given)", args.len()),
                    ));
                }
                let class = cls.borrow();
                let keys: Vec<Value> = class
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
                let class = cls.borrow();
                let vals: Vec<Value> = class.attrs.values().cloned().collect();
                Ok(Value::list(vals))
            }
            "items" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("items() takes no arguments ({} given)", args.len()),
                    ));
                }
                let class = cls.borrow();
                let items: Vec<Value> = class
                    .attrs
                    .iter()
                    .map(|(k, v)| Value::tuple(vec![Value::string(k.clone()), v.clone()]))
                    .collect();
                Ok(Value::list(items))
            }
            "get" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(PyError::named(
                        "TypeError",
                        format!("get() takes 1 or 2 arguments ({} given)", args.len()),
                    ));
                }
                let key_str = match args[0].kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => {
                        // Non-string keys are never in a class dict.
                        return Ok(args.get(1).cloned().unwrap_or(Value::none()));
                    }
                };
                let class = cls.borrow();
                Ok(class
                    .attrs
                    .get(&key_str)
                    .cloned()
                    .unwrap_or_else(|| args.get(1).cloned().unwrap_or(Value::none())))
            }
            "copy" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("copy() takes no arguments ({} given)", args.len()),
                    ));
                }
                let class = cls.borrow();
                let mut dict: IndexMap<PyKey, Value> = IndexMap::new();
                for (k, v) in class.attrs.iter() {
                    dict.insert(PyKey::str_from(k), v.clone());
                }
                Ok(Value::dict(dict))
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'mappingproxy' object has no attribute '{method}'"),
            )),
        }
    }
}

/// Construct a `mappingproxy` Value wrapping a live `PyClass` reference.
pub fn mapping_proxy(class: Rc<RefCell<PyClass>>) -> Value {
    let state: Box<dyn Any> = Box::new(MappingProxyState { class });
    Value::builtin_object(MAPPING_PROXY_OPS, state)
}

/// Extract the inner `Rc<RefCell<PyClass>>` from a mappingproxy Value, or
/// `None` if the value is not a mappingproxy.  Used by `iter_values` in
/// the interpreter so that `for k in vars(Foo)` works without requiring
/// `is_iterable()` / `iter_next` state machinery.
pub fn as_class_rc(value: &Value) -> Option<Rc<RefCell<PyClass>>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    borrow_class(state)
}

fn borrow_class(state: &BuiltinState) -> Option<Rc<RefCell<PyClass>>> {
    let borrow = state.borrow();
    borrow
        .downcast_ref::<MappingProxyState>()
        .map(|s| Rc::clone(&s.class))
}
