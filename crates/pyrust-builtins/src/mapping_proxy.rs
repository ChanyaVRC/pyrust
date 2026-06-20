//! `mappingproxy` — a read-only view of a mapping.
//!
//! Returned by `vars(SomeClass)` / `type(C).__dict__` (proxying a `PyClass`'s
//! attribute dict) and by `d.keys()/.values()/.items().mapping` (proxying the
//! parent dict, issue #2679).  Delegates reads to the live source and raises
//! `TypeError` on any mutation attempt, matching CPython 3.12's
//! `types.MappingProxyType` behaviour.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyClass, PyDict, PyError, PyKey, Result, Value, ValueKind,
    key_repr,
};

pub const TYPE_NAME: &str = "mappingproxy";

/// What a `mappingproxy` is proxying.  Both variants hold a live `Rc`, so reads
/// reflect subsequent mutations of the underlying mapping.
pub enum MappingProxySource {
    /// A class's attribute dict (`vars(C)` / `type(C).__dict__`).  Keys are
    /// always strings.
    Class(Rc<RefCell<PyClass>>),
    /// A plain dict (`d.keys().mapping`, issue #2679).  Keys are arbitrary
    /// hashable values.
    Dict(Rc<RefCell<PyDict>>),
}

/// Internal state: a live reference to the mapping being proxied.
pub struct MappingProxyState {
    pub source: MappingProxySource,
}

pub struct MappingProxyOps;

pub const MAPPING_PROXY_OPS: &MappingProxyOps = &MappingProxyOps;

impl BuiltinTypeOps for MappingProxyOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let src = borrow_source(state).expect("mappingproxy state");
        let inner: Vec<String> = match &src {
            MappingProxySource::Class(cls) => cls
                .borrow()
                .attrs
                .iter()
                .map(|(k, v)| format!("{}: {}", Value::string(k.clone()).repr_raw(), v.repr_raw()))
                .collect(),
            MappingProxySource::Dict(rc) => rc
                .borrow()
                .iter()
                .map(|(k, v)| format!("{}: {}", key_repr(k), v.repr_raw()))
                .collect(),
        };
        format!("mappingproxy({{{}}})", inner.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        match borrow_source(state) {
            Some(MappingProxySource::Class(cls)) => !cls.borrow().attrs.is_empty(),
            Some(MappingProxySource::Dict(rc)) => !rc.borrow().is_empty(),
            None => false,
        }
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let src = match borrow_source(state) {
            Some(s) => s,
            None => return false,
        };
        // mappingproxy compares equal to a dict (or another mappingproxy) with
        // the same content.
        match other.kind() {
            ValueKind::Dict(rhs) => match &src {
                MappingProxySource::Class(cls) => {
                    let class = cls.borrow();
                    if class.attrs.len() != rhs.len() {
                        return false;
                    }
                    class.attrs.iter().all(|(k, v)| {
                        rhs.get(&PyKey::str_from(k))
                            .is_some_and(|other_v| v == other_v)
                    })
                }
                MappingProxySource::Dict(lhs) => {
                    let lhs = lhs.borrow();
                    if lhs.len() != rhs.len() {
                        return false;
                    }
                    lhs.iter()
                        .all(|(k, v)| rhs.get(k).is_some_and(|other_v| v == other_v))
                }
            },
            ValueKind::BuiltinObject {
                ops,
                state: rhs_state,
            } if ops.type_name() == TYPE_NAME => {
                let rhs_src = match borrow_source(rhs_state) {
                    Some(s) => s,
                    None => return false,
                };
                match (&src, &rhs_src) {
                    (MappingProxySource::Class(l), MappingProxySource::Class(r)) => {
                        Rc::ptr_eq(l, r) || l.borrow().attrs == r.borrow().attrs
                    }
                    (MappingProxySource::Dict(l), MappingProxySource::Dict(r)) => {
                        Rc::ptr_eq(l, r) || *l.borrow() == *r.borrow()
                    }
                    // Class-proxy vs dict-proxy: compare content (string keys).
                    (MappingProxySource::Class(c), MappingProxySource::Dict(d))
                    | (MappingProxySource::Dict(d), MappingProxySource::Class(c)) => {
                        let class = c.borrow();
                        let dict = d.borrow();
                        if class.attrs.len() != dict.len() {
                            return false;
                        }
                        class
                            .attrs
                            .iter()
                            .all(|(k, v)| dict.get(&PyKey::str_from(k)).is_some_and(|dv| v == dv))
                    }
                }
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        match borrow_source(state)? {
            MappingProxySource::Class(cls) => Some(cls.borrow().attrs.len()),
            MappingProxySource::Dict(rc) => Some(rc.borrow().len()),
        }
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let src = borrow_source(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        match &src {
            MappingProxySource::Class(cls) => {
                let key_str = match key.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named("KeyError", key.repr_raw())),
                };
                cls.borrow()
                    .attrs
                    .get(&key_str)
                    .cloned()
                    .ok_or_else(|| PyError::named("KeyError", key.repr_raw()))
            }
            MappingProxySource::Dict(rc) => {
                let pk = key
                    .to_key()
                    .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
                rc.borrow()
                    .get(&pk)
                    .cloned()
                    .ok_or_else(|| PyError::named("KeyError", key.repr_raw()))
            }
        }
    }

    // `set_item` raises TypeError — the default BuiltinTypeOps impl already
    // produces "'mappingproxy' object does not support item assignment".

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let src = borrow_source(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        match &src {
            MappingProxySource::Class(cls) => {
                let key_str = match item.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Ok(false),
                };
                Ok(cls.borrow().attrs.contains_key(&key_str))
            }
            MappingProxySource::Dict(rc) => match item.to_key() {
                Some(pk) => Ok(rc.borrow().contains_key(&pk)),
                None => Ok(false),
            },
        }
    }

    fn has_method(&self, name: &str) -> bool {
        matches!(name, "keys" | "values" | "items" | "get" | "copy")
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let src = borrow_source(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        // None of mappingproxy's methods accept keyword arguments in CPython 3.12.
        if !kwargs.is_empty() && matches!(method, "keys" | "values" | "items" | "get" | "copy") {
            return Err(PyError::named(
                "TypeError",
                format!("mappingproxy.{method}() takes no keyword arguments"),
            ));
        }
        match method {
            "keys" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("keys() takes no arguments ({} given)", args.len()),
                    ));
                }
                Ok(Value::list(source_keys(&src)))
            }
            "values" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("values() takes no arguments ({} given)", args.len()),
                    ));
                }
                Ok(Value::list(source_values(&src)))
            }
            "items" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("items() takes no arguments ({} given)", args.len()),
                    ));
                }
                let items: Vec<Value> = source_keys(&src)
                    .into_iter()
                    .zip(source_values(&src))
                    .map(|(k, v)| Value::tuple(vec![k, v]))
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
                let default = || args.get(1).cloned().unwrap_or(Value::none());
                match &src {
                    MappingProxySource::Class(cls) => {
                        let key_str = match args[0].kind() {
                            ValueKind::Str(s) => s.to_string(),
                            // Non-string keys are never in a class dict.
                            _ => return Ok(default()),
                        };
                        Ok(cls
                            .borrow()
                            .attrs
                            .get(&key_str)
                            .cloned()
                            .unwrap_or_else(default))
                    }
                    MappingProxySource::Dict(rc) => match args[0].to_key() {
                        Some(pk) => Ok(rc.borrow().get(&pk).cloned().unwrap_or_else(default)),
                        None => Ok(default()),
                    },
                }
            }
            "copy" => {
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("copy() takes no arguments ({} given)", args.len()),
                    ));
                }
                match &src {
                    MappingProxySource::Class(cls) => {
                        let class = cls.borrow();
                        let mut dict: PyDict = PyDict::default();
                        for (k, v) in class.attrs.iter() {
                            dict.insert(PyKey::str_from(k), v.clone());
                        }
                        Ok(Value::dict(dict))
                    }
                    MappingProxySource::Dict(rc) => Ok(Value::dict(rc.borrow().clone())),
                }
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'mappingproxy' object has no attribute '{method}'"),
            )),
        }
    }
}

/// The keys of a proxy source as `Value`s, preserving insertion order.
fn source_keys(src: &MappingProxySource) -> Vec<Value> {
    match src {
        MappingProxySource::Class(cls) => cls
            .borrow()
            .attrs
            .keys()
            .map(|k| Value::string(k.clone()))
            .collect(),
        MappingProxySource::Dict(rc) => rc.borrow().keys().map(key_to_value).collect(),
    }
}

/// The values of a proxy source, preserving insertion order.
fn source_values(src: &MappingProxySource) -> Vec<Value> {
    match src {
        MappingProxySource::Class(cls) => cls.borrow().attrs.values().cloned().collect(),
        MappingProxySource::Dict(rc) => rc.borrow().values().cloned().collect(),
    }
}

/// Reconstruct a `Value` from a `PyKey`.  Mirrors the interpreter's
/// `helpers::key_to_value`; kept local so `pyrust-builtins` needs no interpreter
/// dependency.  Every `PyKey` variant must be handled.
fn key_to_value(key: &PyKey) -> Value {
    match key.clone() {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(v) => Value::float(f64::from_bits(v)),
        PyKey::Str(v) => v,
        PyKey::Bool(v) => Value::bool_(v),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(items) => {
            let mut set: pyrust_core::PySet = pyrust_core::PySet::default();
            for k in items {
                set.insert(k);
            }
            crate::frozenset::frozenset(set)
        }
        PyKey::Tuple(items) => Value::tuple(items.iter().map(key_to_value).collect()),
        PyKey::Bytes(rc) => Value::bytes((*rc).clone()),
        PyKey::Complex(re, im) => Value::complex(re, im),
        PyKey::Object { value, .. } => value,
    }
}

/// Construct a `mappingproxy` Value wrapping a live `PyClass` reference.
pub fn mapping_proxy(class: Rc<RefCell<PyClass>>) -> Value {
    let state: Box<dyn Any> = Box::new(MappingProxyState {
        source: MappingProxySource::Class(class),
    });
    Value::builtin_object(MAPPING_PROXY_OPS, state)
}

/// Construct a `mappingproxy` Value wrapping a live dict (issue #2679).
pub fn mapping_proxy_dict(dict: Rc<RefCell<PyDict>>) -> Value {
    let state: Box<dyn Any> = Box::new(MappingProxyState {
        source: MappingProxySource::Dict(dict),
    });
    Value::builtin_object(MAPPING_PROXY_OPS, state)
}

/// Extract the inner `Rc<RefCell<PyClass>>` from a class-backed mappingproxy
/// Value, or `None` if the value is not a class-backed mappingproxy.  Used by
/// `iter_values` in the interpreter so that `for k in vars(Foo)` works without
/// requiring `is_iterable()` / `iter_next` state machinery.
pub fn as_class_rc(value: &Value) -> Option<Rc<RefCell<PyClass>>> {
    match borrow_source_of(value)? {
        MappingProxySource::Class(c) => Some(c),
        MappingProxySource::Dict(_) => None,
    }
}

/// Extract the inner dict `Rc` from a dict-backed mappingproxy Value, or `None`
/// if the value is not a dict-backed mappingproxy (issue #2679).
pub fn as_dict_rc(value: &Value) -> Option<Rc<RefCell<PyDict>>> {
    match borrow_source_of(value)? {
        MappingProxySource::Dict(d) => Some(d),
        MappingProxySource::Class(_) => None,
    }
}

fn borrow_source_of(value: &Value) -> Option<MappingProxySource> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    borrow_source(state)
}

/// Clone the live `Rc` out of the proxy state.  Both variants are cheap `Rc`
/// bumps; the borrow is released before returning.
fn borrow_source(state: &BuiltinState) -> Option<MappingProxySource> {
    let borrow = state.borrow();
    borrow
        .downcast_ref::<MappingProxyState>()
        .map(|s| match &s.source {
            MappingProxySource::Class(c) => MappingProxySource::Class(Rc::clone(c)),
            MappingProxySource::Dict(d) => MappingProxySource::Dict(Rc::clone(d)),
        })
}
