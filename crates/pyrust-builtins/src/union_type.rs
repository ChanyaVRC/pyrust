//! `UnionType` — the result of `X | Y` on type objects.
//!
//! PEP 604 (Python 3.10+) lets you write `int | str` as a union type usable
//! in `isinstance()`, `issubclass()`, and annotations.  `type.__or__` returns
//! a `types.UnionType` carrying a flat tuple of component types.
//!
//! This module provides the pyrust equivalent as a `BuiltinTypeOps`
//! implementation backed by `UnionTypeState { args }`.

use std::any::Any;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use pyrust_core::{BuiltinState, BuiltinTypeOps, PyKey, Value, ValueKind};

pub struct UnionTypeState {
    /// The component types, as a flat tuple.  Matches CPython's `__args__`.
    pub args: Value,
}

pub struct UnionTypeOps;

pub const UNION_TYPE_OPS: &UnionTypeOps = &UnionTypeOps;
pub const TYPE_NAME: &str = "types.UnionType";

impl BuiltinTypeOps for UnionTypeOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<UnionTypeState>()
            .expect("UnionTypeOps: bad state");

        if let ValueKind::Tuple(items) = s.args.kind() {
            items
                .iter()
                .map(repr_type_component)
                .collect::<Vec<_>>()
                .join(" | ")
        } else {
            repr_type_component(&s.args)
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<UnionTypeState>()?;
        match name {
            "__args__" => Some(s.args.clone()),
            _ => None,
        }
    }

    /// Two `UnionType` values are equal iff they contain the same set of types,
    /// regardless of order (CPython semantics: `int | str == str | int` is `True`).
    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let borrow = state.borrow();
        let s = match borrow.downcast_ref::<UnionTypeState>() {
            Some(s) => s,
            None => return false,
        };
        if let ValueKind::BuiltinObject {
            ops: other_ops,
            state: other_state,
        } = other.kind()
        {
            if other_ops.type_name() != TYPE_NAME {
                return false;
            }
            let other_borrow = other_state.borrow();
            let other_s = match other_borrow.downcast_ref::<UnionTypeState>() {
                Some(s) => s,
                None => return false,
            };
            // Compare as sets: same elements, regardless of order.
            args_eq_as_set(&s.args, &other_s.args)
        } else {
            false
        }
    }

    /// `hash(int | str)` — CPython computes this as `hash(frozenset(__args__))`.
    /// We approximate by hashing the XOR of sorted component hashes.
    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<UnionTypeState>()?;
        hash_args(&s.args)
    }

    fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
        let combined = self.hash(state)?;
        let value = Value::builtin_object_shared(UNION_TYPE_OPS, state.clone());
        Some(PyKey::Object {
            hash: combined,
            value,
        })
    }
}

/// Repr for a single component of a union type.
/// For a class this is the qualified name.
/// For a `NoneType` token it is `None` (matching CPython: `int | None` not `int | NoneType`).
/// For a nested UnionType we recursively repr it.
pub fn repr_type_component(v: &Value) -> String {
    match v.kind() {
        ValueKind::PyClass(rc) => rc.borrow().qualname.clone(),
        ValueKind::BuiltinFunction("NoneType") => "None".to_string(),
        ValueKind::BuiltinObject { ops, state } if ops.type_name() == TYPE_NAME => ops.repr(state),
        _ => v.repr(),
    }
}

/// Compare two `__args__` tuples as unordered sets.
fn args_eq_as_set(a: &Value, b: &Value) -> bool {
    let a_items = match a.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => return a == b,
    };
    let b_items = match b.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => return false,
    };
    if a_items.len() != b_items.len() {
        return false;
    }
    // Check that every item in a appears in b (O(n^2) but unions are tiny).
    for ai in &a_items {
        if !b_items.iter().any(|bi| ai == bi) {
            return false;
        }
    }
    true
}

/// Hash the args tuple as a set (order-independent).
fn hash_args(args: &Value) -> Option<u64> {
    let items = match args.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => return None,
    };
    // Sort hashes and XOR-accumulate so the result is order-independent.
    let mut hashes: Vec<u64> = items.iter().map(component_hash).collect::<Option<_>>()?;
    hashes.sort_unstable();
    let mut xor: u64 = 0;
    for v in hashes {
        xor ^= v;
    }
    // Mix to spread bits.
    let mut hasher = DefaultHasher::new();
    xor.hash(&mut hasher);
    Some(hasher.finish())
}

/// Stable hash for a single union component (type value).
fn component_hash(v: &Value) -> Option<u64> {
    if let ValueKind::PyClass(rc) = v.kind() {
        let ptr = std::rc::Rc::as_ptr(rc) as u64;
        let mut h = DefaultHasher::new();
        ptr.hash(&mut h);
        return Some(h.finish());
    }
    if let ValueKind::BuiltinFunction(name) = v.kind() {
        let mut h = DefaultHasher::new();
        name.hash(&mut h);
        return Some(h.finish());
    }
    None
}

/// Construct a `UnionType` value by combining `lhs` and `rhs`.
///
/// Flattens any `UnionType` components so `(int | str) | float` becomes
/// `int | str | float` rather than a nested union.
pub fn make_union_type(lhs: Value, rhs: Value) -> Value {
    let mut components: Vec<Value> = Vec::new();
    collect_union_components(lhs, &mut components);
    collect_union_components(rhs, &mut components);
    let args = Value::tuple(components);
    let state: Box<dyn Any> = Box::new(UnionTypeState { args });
    Value::builtin_object(UNION_TYPE_OPS, state)
}

/// Push all the leaf type components of `v` into `out`, unwrapping nested
/// `UnionType` values so the result stays flat.
fn collect_union_components(v: Value, out: &mut Vec<Value>) {
    if let ValueKind::BuiltinObject { ops, state } = v.kind() {
        if ops.type_name() == TYPE_NAME {
            let borrow = state.borrow();
            if let Some(s) = borrow.downcast_ref::<UnionTypeState>() {
                if let ValueKind::Tuple(items) = s.args.kind() {
                    for item in items.iter() {
                        out.push(item.clone());
                    }
                    return;
                }
            }
        }
    }
    out.push(v);
}

/// True if `v` is a `UnionType` `BuiltinObject`.
pub fn is_union_type(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::BuiltinObject { ops, .. } if ops.type_name() == TYPE_NAME
    )
}

/// Return the `__args__` tuple from a `UnionType` value, or `None` if `v`
/// is not a `UnionType`.
pub fn union_type_args(v: &Value) -> Option<Value> {
    if let ValueKind::BuiltinObject { ops, state } = v.kind() {
        if ops.type_name() == TYPE_NAME {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<UnionTypeState>()?;
            return Some(s.args.clone());
        }
    }
    None
}
