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
    BuiltinState, BuiltinTypeOps, CanonicalClassTag, PyClass, PyDict, PyError, PyKey, Result,
    Value, ValueKind, builtin_ops_is, key_repr,
};

pub const TYPE_NAME: &str = "mappingproxy";

/// What a `mappingproxy` is proxying. Exact dicts and class namespaces retain
/// their direct-storage fast paths; every other object is authoritative and
/// must be read through the interpreter's normal mapping protocols.
pub enum MappingProxySource {
    /// A class's attribute dict (`vars(C)` / `type(C).__dict__`).  Keys are
    /// always strings.
    Class(Rc<RefCell<PyClass>>),
    /// A plain dict (`d.keys().mapping`, issue #2679).  Keys are arbitrary
    /// hashable values.
    Dict(Rc<RefCell<PyDict>>),
    /// Any non-exact mapping accepted by `types.MappingProxyType`. Keeping the
    /// object itself (rather than a discovered backing dict) preserves its
    /// Python-level `__getitem__`, `__len__`, `__iter__`, and named methods.
    Object(Value),
}

/// Internal state: the authoritative target being proxied.
pub struct MappingProxyState {
    pub source: MappingProxySource,
}

pub struct MappingProxyOps;
pub struct MappingProxyObjectOps;

pub const MAPPING_PROXY_OPS: &MappingProxyOps = &MappingProxyOps;
pub const MAPPING_PROXY_OBJECT_OPS: &MappingProxyObjectOps = &MappingProxyObjectOps;

/// Fast tag checks for interpreter dispatch. Object-backed proxies require
/// Python protocol callbacks, while exact dict/class proxies stay entirely in
/// the receiver-only ops table.
#[inline]
pub fn is_exact_proxy_ops(ops: &dyn BuiltinTypeOps) -> bool {
    builtin_ops_is::<MappingProxyOps>(ops)
}

#[inline]
pub fn is_object_proxy_ops(ops: &dyn BuiltinTypeOps) -> bool {
    builtin_ops_is::<MappingProxyObjectOps>(ops)
}

/// Validate the public mappingproxy method wrapper before any delegated owner
/// lookup runs. Exact and object-backed proxies share this boundary so their
/// TypeError precedence and wording cannot drift.
pub fn validate_method_call(method: &str, args_len: usize, has_kwargs: bool) -> Result<()> {
    if has_kwargs
        && matches!(
            method,
            "keys" | "values" | "items" | "get" | "copy" | "__reversed__"
        )
    {
        return Err(PyError::named(
            "TypeError",
            format!("mappingproxy.{method}() takes no keyword arguments"),
        ));
    }
    match method {
        "get" if args_len == 0 => Err(PyError::named(
            "TypeError",
            "get expected at least 1 argument, got 0".to_string(),
        )),
        "get" if args_len > 2 => Err(PyError::named(
            "TypeError",
            format!("get expected at most 2 arguments, got {args_len}"),
        )),
        "keys" | "values" | "items" | "copy" | "__reversed__" if args_len != 0 => {
            Err(PyError::named(
                "TypeError",
                format!("mappingproxy.{method}() takes no arguments ({args_len} given)"),
            ))
        }
        _ => Ok(()),
    }
}

impl BuiltinTypeOps for MappingProxyOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn canonical_class_tag(&self) -> Option<CanonicalClassTag> {
        Some(CanonicalClassTag::MappingProxy)
    }

    fn repr(&self, state: &BuiltinState) -> String {
        // CPython renders `mappingproxy(%R)` over the *proxied object*, so a
        // proxy over an `OrderedDict` reads `mappingproxy(OrderedDict({...}))`
        // (issue #2936).  A `PyInstance` owner needs interpreter dispatch to
        // render, so `render_value_repr` intercepts this case before the
        // built-in ops table is consulted; the `repr_raw` here is the
        // no-interpreter fallback and is exact for a nested `mappingproxy`.
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
            MappingProxySource::Object(owner) => {
                return format!("mappingproxy({})", owner.repr_raw());
            }
        };
        format!("mappingproxy({{{}}})", inner.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        match borrow_source(state) {
            Some(MappingProxySource::Class(cls)) => !cls.borrow().attrs.is_empty(),
            Some(MappingProxySource::Dict(rc)) => !rc.borrow().is_empty(),
            // Interpreter::truthy_value delegates to the object's length slot.
            Some(MappingProxySource::Object(_)) => false,
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
                // Interpreter-side rich comparison unwraps object targets before
                // this receiver-only fallback is reached.
                MappingProxySource::Object(_) => false,
            },
            ValueKind::BuiltinObject {
                ops,
                state: rhs_state,
            } if builtin_ops_is::<MappingProxyOps>(ops) => {
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
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        match borrow_source(state)? {
            MappingProxySource::Class(cls) => Some(cls.borrow().attrs.len()),
            MappingProxySource::Dict(rc) => Some(rc.borrow().len()),
            MappingProxySource::Object(_) => None,
        }
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let src = borrow_source(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        match &src {
            MappingProxySource::Class(cls) => {
                let key_str = match key.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::key_error(key.clone())),
                };
                let value = cls.borrow().attrs.get(&key_str).cloned();
                value
                    .map(|value| expose_class_value(&value))
                    .ok_or_else(|| PyError::key_error(key.clone()))
            }
            MappingProxySource::Dict(rc) => {
                let pk = key
                    .to_key()
                    .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
                rc.borrow()
                    .get(&pk)
                    .cloned()
                    .ok_or_else(|| PyError::key_error(key.clone()))
            }
            MappingProxySource::Object(_) => Err(PyError::Runtime(
                "internal: mappingproxy object read requires interpreter dispatch".to_string(),
            )),
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
            MappingProxySource::Object(_) => Err(PyError::Runtime(
                "internal: mappingproxy object membership requires interpreter dispatch"
                    .to_string(),
            )),
        }
    }

    fn has_method(&self, name: &str) -> bool {
        matches!(
            name,
            "keys" | "values" | "items" | "get" | "copy" | "__reversed__"
        )
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        validate_method_call(method, args.len(), !kwargs.is_empty())?;
        let src = borrow_source(state)
            .ok_or_else(|| PyError::Runtime("internal: bad mappingproxy state".to_string()))?;
        if matches!(&src, MappingProxySource::Object(_)) {
            return Err(PyError::Runtime(
                "internal: mappingproxy object method requires interpreter dispatch".to_string(),
            ));
        }
        match method {
            "keys" => Ok(crate::dict_views::dict_keys(source_dict_rc(&src))),
            "values" => Ok(crate::dict_views::dict_values(source_dict_rc(&src))),
            "items" => Ok(crate::dict_views::dict_items(source_dict_rc(&src))),
            "get" => {
                let default = || args.get(1).cloned().unwrap_or(Value::none());
                match &src {
                    MappingProxySource::Class(cls) => {
                        let key_str = match args[0].kind() {
                            ValueKind::Str(s) => s.to_string(),
                            // Non-string keys are never in a class dict.
                            _ => return Ok(default()),
                        };
                        let value = cls.borrow().attrs.get(&key_str).cloned();
                        Ok(value
                            .map(|value| expose_class_value(&value))
                            .unwrap_or_else(default))
                    }
                    MappingProxySource::Dict(rc) => match args[0].to_key() {
                        Some(pk) => Ok(rc.borrow().get(&pk).cloned().unwrap_or_else(default)),
                        None => Ok(default()),
                    },
                    MappingProxySource::Object(_) => unreachable!(),
                }
            }
            "copy" => match &src {
                MappingProxySource::Class(cls) => {
                    let class = cls.borrow();
                    let mut dict: PyDict = PyDict::default();
                    for (k, v) in class.attrs.iter() {
                        dict.insert(PyKey::str_from(k), expose_class_value(v));
                    }
                    Ok(Value::dict(dict))
                }
                MappingProxySource::Dict(rc) => Ok(Value::dict(rc.borrow().clone())),
                MappingProxySource::Object(_) => unreachable!(),
            },
            "__reversed__" => {
                // Yield keys in reverse insertion order (CPython 3.12 #2684).
                // Reported as `dict_reversekeyiterator` to match CPython's
                // kind-specific iterator type name (#2702).
                let keys = source_keys(&src);
                Ok(crate::iter_helpers::reversed_dict_keys(Value::list(keys)))
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'mappingproxy' object has no attribute '{method}'"),
            )),
        }
    }
}

// Object-backed proxies need interpreter callbacks for reads. A distinct ops
// type lets the interpreter reject exact dict/class proxies by type id without
// borrowing their state on every hot read; fallback presentation and mutation
// behavior remain shared with MappingProxyOps.
impl BuiltinTypeOps for MappingProxyObjectOps {
    fn type_name(&self) -> &'static str {
        MAPPING_PROXY_OPS.type_name()
    }

    fn canonical_class_tag(&self) -> Option<CanonicalClassTag> {
        MAPPING_PROXY_OPS.canonical_class_tag()
    }

    fn repr(&self, state: &BuiltinState) -> String {
        MAPPING_PROXY_OPS.repr(state)
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        MAPPING_PROXY_OPS.truthy(state)
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        MAPPING_PROXY_OPS.eq(state, other)
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        MAPPING_PROXY_OPS.len(state)
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        MAPPING_PROXY_OPS.get_item(state, key)
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        MAPPING_PROXY_OPS.contains(state, item)
    }

    fn has_method(&self, name: &str) -> bool {
        MAPPING_PROXY_OPS.has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        MAPPING_PROXY_OPS.call_method(state, method, args, kwargs)
    }
}

/// A live `Rc<RefCell<PyDict>>` over the proxy source, for vending `dict_keys` /
/// `dict_values` / `dict_items` views (issue #2751).  Matches CPython 3.12,
/// where `mappingproxy.keys()/values()/items()` return the underlying dict's
/// own view types rather than snapshot lists.
///
/// - `Dict`: returns the live backing rc, so the view reflects later mutations
///   and carries the size-mutation guard.
/// - `Class`: a class's attribute store is an `IndexMap<String, Value>`, not a
///   `PyDict`, so there is no live rc to share; we snapshot the current attrs
///   into a fresh dict rc.  The resulting view still has the correct
///   `dict_keys`/`dict_values`/`dict_items` type identity and mutation guard.
fn source_dict_rc(src: &MappingProxySource) -> Rc<RefCell<PyDict>> {
    match src {
        MappingProxySource::Dict(rc) => Rc::clone(rc),
        MappingProxySource::Class(cls) => {
            let class = cls.borrow();
            let mut dict = PyDict::default();
            for (k, v) in class.attrs.iter() {
                dict.insert(PyKey::str_from(k), expose_class_value(v));
            }
            Rc::new(RefCell::new(dict))
        }
        MappingProxySource::Object(_) => {
            unreachable!("object-backed mappingproxy methods dispatch in the interpreter")
        }
    }
}

/// Class dictionaries internally keep member descriptors cycle-free. Every
/// Python-visible mappingproxy read must vend the detached, owner-retaining
/// descriptor form.
fn expose_class_value(value: &Value) -> Value {
    crate::member_descriptor::export_member_descriptor(value).unwrap_or_else(|| value.clone())
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
        MappingProxySource::Object(_) => {
            unreachable!("object-backed mappingproxy iteration dispatches in the interpreter")
        }
    }
}

/// Reconstruct a `Value` from a `PyKey`.  Mirrors the interpreter's
/// `helpers::key_to_value`; kept local so `pyrust-builtins` needs no interpreter
/// dependency.  Every `PyKey` variant must be handled.
fn key_to_value(key: &PyKey) -> Value {
    match key.clone() {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(v) => Value::float_from_bits(v),
        PyKey::Str(v) => v,
        PyKey::Bool(v) => Value::bool_(v),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(key) => crate::frozenset::frozenset_key(key),
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

/// Construct a `mappingproxy` over an arbitrary object. The interpreter
/// delegates every read to this exact object; nested proxies intentionally
/// retain each wrapper instead of being flattened.
pub fn mapping_proxy_object(owner: Value) -> Value {
    let state: Box<dyn Any> = Box::new(MappingProxyState {
        source: MappingProxySource::Object(owner),
    });
    Value::builtin_object(MAPPING_PROXY_OBJECT_OPS, state)
}

/// The proxied object of an owner-carrying `mappingproxy`, or `None` for a
/// proxy over a plain dict or a class `__dict__` (where the source *is* the
/// proxied object).
#[inline]
pub fn owner_of(value: &Value) -> Option<Value> {
    // Tag-only pre-check: operator dispatch calls this on every operand, so the
    // common non-`BuiltinObject` case must not pay for building a `ValueKind`.
    if !value.is_builtin_object() {
        return None;
    }
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !is_object_proxy_ops(ops) {
        return None;
    }
    owner_from_state(state)
}

/// Clone an object-backed proxy's authoritative owner directly from an
/// already-matched `BuiltinObject` state. The state borrow is released before
/// any interpreter callback runs.
#[inline]
pub fn owner_from_state(state: &BuiltinState) -> Option<Value> {
    match borrow_source(state)? {
        MappingProxySource::Object(owner) => Some(owner),
        MappingProxySource::Class(_) | MappingProxySource::Dict(_) => None,
    }
}

/// The object an owner-carrying `mappingproxy` ultimately proxies, following a
/// chain of nested proxies to its end.
///
/// [`owner_of`] deliberately stops after one hop, because `repr` nests one
/// `mappingproxy(...)` wrapper per level.  The operator arms need the opposite:
/// CPython's `mappingproxy_richcompare` and `mappingproxy_or` forward to
/// `pp->mapping`, and when that is itself a proxy the forwarded call forwards
/// again — so `mappingproxy(mappingproxy(od)) == od` compares `od == od`, and
/// `mappingproxy(mappingproxy(counter)) | other` keeps `Counter`'s multiset
/// `__or__`.  Resolving only one hop left the *inner proxy* as the operand,
/// which falls back to plain-dict semantics and loses both the equality and
/// the proxied type (issue #2936 review).
///
/// Returns `None` for a proxy with no owner at all (a plain dict or a class
/// `__dict__`, where the source *is* the proxied object).
///
/// `#[inline]` for the same reason as [`owner_of`]: the `Eq` / `Ne` / `BitOr`
/// arms call this on every operand, so the tag-only rejection must fold into
/// the caller rather than cost a call.
#[inline]
pub fn proxied_of(value: &Value) -> Option<Value> {
    // Tag-only pre-check: operator dispatch calls this on every operand, so the
    // common non-`BuiltinObject` case must not pay for building a `ValueKind`.
    if !value.is_builtin_object() {
        return None;
    }
    let mut proxied = owner_of(value)?;
    // Each hop is a `mappingproxy` built over another `mappingproxy`; the chain
    // is as deep as the nesting the program wrote, and ends at the first
    // non-proxy (or owner-less) value.
    while let Some(next) = owner_of(&proxied) {
        proxied = next;
    }
    Some(proxied)
}

/// Return whether `value` is backed by this module's mappingproxy operations
/// table. The check uses the concrete Rust implementation type, never the
/// Python-visible `mappingproxy` presentation name.
#[inline]
pub fn is_mapping_proxy(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::BuiltinObject { ops, .. }
            if is_exact_proxy_ops(ops) || is_object_proxy_ops(ops)
    )
}

/// Extract the inner `Rc<RefCell<PyClass>>` from a class-backed mappingproxy
/// Value, or `None` if the value is not a class-backed mappingproxy.  Used by
/// `iter_values` in the interpreter so that `for k in vars(Foo)` works without
/// requiring `is_iterable()` / `iter_next` state machinery.
pub fn as_class_rc(value: &Value) -> Option<Rc<RefCell<PyClass>>> {
    match borrow_source_of(value)? {
        MappingProxySource::Class(c) => Some(c),
        MappingProxySource::Dict(_) | MappingProxySource::Object(_) => None,
    }
}

/// Extract the inner dict `Rc` from a dict-backed mappingproxy Value, or `None`
/// if the value is not a dict-backed mappingproxy (issue #2679).
pub fn as_dict_rc(value: &Value) -> Option<Rc<RefCell<PyDict>>> {
    match borrow_source_of(value)? {
        MappingProxySource::Dict(d) => Some(d),
        MappingProxySource::Class(_) | MappingProxySource::Object(_) => None,
    }
}

/// Live element count of any (class- or dict-backed) mappingproxy `Value`, or
/// `None` if `value` is not a mappingproxy.  Used by the interpreter's
/// `live_collection_len` so iterators over a mappingproxy install a size-mutation
/// guard (issue #2728): the count is re-read each `next()` step and a change
/// raises `RuntimeError: dictionary changed size during iteration`.
pub fn live_len(value: &Value) -> Option<usize> {
    match borrow_source_of(value)? {
        MappingProxySource::Class(cls) => Some(cls.borrow().attrs.len()),
        MappingProxySource::Dict(rc) => Some(rc.borrow().len()),
        MappingProxySource::Object(_) => None,
    }
}

fn borrow_source_of(value: &Value) -> Option<MappingProxySource> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !is_exact_proxy_ops(ops) && !is_object_proxy_ops(ops) {
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
            MappingProxySource::Object(owner) => MappingProxySource::Object(owner.clone()),
        })
}
