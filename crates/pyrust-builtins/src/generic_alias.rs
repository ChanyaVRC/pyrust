//! `GenericAlias` value — returned by `list[int]`, `dict[str, int]`, etc.
//!
//! PEP 585 (Python 3.9+) lets you write `list[int]` instead of
//! `typing.List[int]`.  Built-in collection types expose `__class_getitem__`
//! as a classmethod; subscripting them creates a `types.GenericAlias` that
//! carries `(__origin__, __args__)` and has a human-readable repr.
//!
//! This module provides the pyrust equivalent as a `BuiltinTypeOps`
//! implementation backed by `GenericAliasState { origin, args }`.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, ValueKind};

pub struct GenericAliasState {
    /// The origin type (e.g. the `list` class value).
    pub origin: Value,
    /// The type argument(s).  For a single-arg subscript (`list[int]`) this is
    /// a one-element tuple; for multi-arg (`dict[str, int]`) it is a two-(or
    /// more)-element tuple.  Matches CPython's `GenericAlias.__args__`.
    pub args: Value,
}

pub struct GenericAliasOps;

pub const GENERIC_ALIAS_OPS: &GenericAliasOps = &GenericAliasOps;
pub const TYPE_NAME: &str = "types.GenericAlias";

impl BuiltinTypeOps for GenericAliasOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<GenericAliasState>()
            .expect("GenericAliasOps: bad state");

        // Derive the origin name: prefer `qualname` for PyClass, fall back to
        // the builtin_type_name helper for any other kind of origin.
        let origin_name = match s.origin.kind() {
            ValueKind::PyClass(rc) => rc.borrow().qualname.clone(),
            _ => pyrust_core::builtin_type_name(&s.origin).into_owned(),
        };

        // args is always a tuple of one or more elements.
        let args_repr = match s.args.kind() {
            ValueKind::Tuple(items) => items
                .iter()
                .map(repr_type_arg)
                .collect::<Vec<_>>()
                .join(", "),
            // Fallback: shouldn't happen in normal use, but handle gracefully.
            _ => repr_type_arg(&s.args),
        };

        format!("{origin_name}[{args_repr}]")
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<GenericAliasState>()?;
        match name {
            "__origin__" => Some(s.origin.clone()),
            "__args__" => Some(s.args.clone()),
            _ => None,
        }
    }
}

/// Produce the repr for a single type argument, matching how CPython formats
/// `GenericAlias.__repr__`.  For a class this is just the qualified name
/// (e.g. `"int"`, `"str"`).  For nested GenericAlias values (e.g.
/// `list[list[int]]`) it recursively produces `"list[int]"`.  For anything
/// else we fall back to the general `Value::repr()`.
fn repr_type_arg(v: &Value) -> String {
    match v.kind() {
        ValueKind::PyClass(rc) => rc.borrow().qualname.clone(),
        ValueKind::BuiltinObject { ops, state } if ops.type_name() == TYPE_NAME => ops.repr(state),
        _ => v.repr(),
    }
}

/// Construct a `GenericAlias` value.
///
/// `origin` should be the subscripted class value.
/// `args` should be a tuple of type arguments (always a tuple, even for a
/// single argument — matches CPython's `GenericAlias.__args__` contract).
pub fn generic_alias(origin: Value, args: Value) -> Value {
    let state: Box<dyn Any> = Box::new(GenericAliasState { origin, args });
    Value::builtin_object(GENERIC_ALIAS_OPS, state)
}
