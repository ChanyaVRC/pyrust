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

use pyrust_core::{
    BuiltinState, BuiltinTypeOps, CanonicalClassTag, PyKey, Value, ValueKind, builtin_ops_is,
};

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
            if !builtin_ops_is::<UnionTypeOps>(other_ops) {
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
/// `NoneType` displays as `None` in union repr (CPython: `int | None`, not `int | NoneType`).
/// For a nested UnionType we recursively repr it.
pub fn repr_type_component(v: &Value) -> String {
    match v.kind() {
        ValueKind::PyClass(rc) => {
            let class = rc.borrow();
            // `NoneType` displays as `None` in union repr (CPython: `int | None`).
            if class.canonical_tag == Some(CanonicalClassTag::NoneType) {
                "None".to_string()
            } else {
                class.qualname.clone()
            }
        }
        ValueKind::BuiltinObject { ops, state } if builtin_ops_is::<UnionTypeOps>(ops) => {
            ops.repr(state)
        }
        _ => v.repr_raw(),
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
    if matches!(v.kind(), ValueKind::BuiltinFunction(_)) {
        let mut h = DefaultHasher::new();
        let function = v.as_function_rc()?;
        (std::rc::Rc::as_ptr(function) as usize).hash(&mut h);
        return Some(h.finish());
    }
    None
}

/// Construct a `UnionType` value by combining `lhs` and `rhs`.
///
/// Flattens any `UnionType` components so `(int | str) | float` becomes
/// `int | str | float` rather than a nested union.  Deduplicates identical
/// type components (CPython: `int | int` returns `int`, not a `UnionType`).
/// When deduplication leaves a single component the component is returned
/// directly, exactly as CPython does (`int | int is int` is `True`).
pub fn make_union_type(lhs: Value, rhs: Value) -> Value {
    let mut components: Vec<Value> = Vec::new();
    collect_union_components(lhs, &mut components);
    collect_union_components(rhs, &mut components);
    // Deduplicate: keep only the first occurrence of each type identity.
    let mut deduped: Vec<Value> = Vec::with_capacity(components.len());
    for v in components {
        if !deduped.iter().any(|u| same_type_identity(u, &v)) {
            deduped.push(v);
        }
    }
    // If deduplication collapses to a single type, return it directly —
    // CPython: `int | int is int` is `True`.
    if deduped.len() == 1 {
        return deduped.into_iter().next().unwrap();
    }
    let args = Value::tuple(deduped);
    let state: Box<dyn Any> = Box::new(UnionTypeState { args });
    Value::builtin_object(UNION_TYPE_OPS, state)
}

/// Return `true` if `a` and `b` are the same type identity, used for
/// deduplication in union construction.
///
/// - `PyClass` identity: same `Rc` pointer (same class object).
/// - `BuiltinFunction` identity: same concrete function-object Rc.
fn same_type_identity(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::PyClass(ra), ValueKind::PyClass(rb)) => std::rc::Rc::ptr_eq(ra, rb),
        (ValueKind::BuiltinFunction(_), ValueKind::BuiltinFunction(_)) => {
            match (a.as_function_rc(), b.as_function_rc()) {
                (Some(ra), Some(rb)) => std::rc::Rc::ptr_eq(ra, rb),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Push all the leaf type components of `v` into `out`, unwrapping nested
/// `UnionType` values so the result stays flat.
fn collect_union_components(v: Value, out: &mut Vec<Value>) {
    if let ValueKind::BuiltinObject { ops, state } = v.kind()
        && builtin_ops_is::<UnionTypeOps>(ops)
    {
        let borrow = state.borrow();
        if let Some(s) = borrow.downcast_ref::<UnionTypeState>()
            && let ValueKind::Tuple(items) = s.args.kind()
        {
            for item in items.iter() {
                out.push(item.clone());
            }
            return;
        }
    }
    out.push(v);
}

/// True if `v` is a `UnionType` `BuiltinObject`.
pub fn is_union_type(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::BuiltinObject { ops, .. } if builtin_ops_is::<UnionTypeOps>(ops)
    )
}

/// Return the `__args__` tuple from a `UnionType` value, or `None` if `v`
/// is not a `UnionType`.
pub fn union_type_args(v: &Value) -> Option<Value> {
    if let ValueKind::BuiltinObject { ops, state } = v.kind()
        && builtin_ops_is::<UnionTypeOps>(ops)
    {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<UnionTypeState>()?;
        return Some(s.args.clone());
    }
    None
}
