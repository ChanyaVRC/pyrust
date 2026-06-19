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
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use pyrust_core::{BuiltinState, BuiltinTypeOps, PyKey, Value, ValueKind};

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
        //
        // CPython's `ga_repr` qualifies a class origin with its module
        // (`{__module__}.{__qualname__}`) unless the module is `builtins` (or
        // absent), so `class D[T]` renders as `__main__.D[int]` while
        // `list[int]` stays bare.  We mirror that by reading `__module__` from
        // the origin class's own dict.
        let origin_name = match s.origin.kind() {
            ValueKind::PyClass(rc) => {
                let c = rc.borrow();
                let module = c
                    .attrs
                    .get("__module__")
                    .and_then(|m| m.as_str().map(|s| s.to_string()));
                match module {
                    Some(m) if m != "builtins" && !m.is_empty() => {
                        format!("{m}.{}", c.qualname)
                    }
                    _ => c.qualname.clone(),
                }
            }
            _ => pyrust_core::builtin_type_name(&s.origin).into_owned(),
        };

        // `typing.Union[X, NoneType]` (exactly two args, one of them
        // `NoneType`) renders as `typing.Optional[X]`, mirroring CPython's
        // `_SpecialForm`/`_GenericAlias.__repr__` for unions.  The flatten
        // helper in `typing.rs` always lowers `Optional[...]` to a `Union`
        // origin, so this is the single place the `Optional` spelling is
        // reconstructed.
        if origin_name == "typing.Union"
            && let ValueKind::Tuple(items) = s.args.kind()
            && items.len() == 2
        {
            let none_pos = items.iter().position(is_none_type_class);
            if let Some(pos) = none_pos {
                let other = &items[1 - pos];
                return format!("typing.Optional[{}]", repr_type_arg(other));
            }
        }

        // args is a tuple; for `tuple[()]` it is empty, which CPython's
        // `ga_repr` renders as `()` (so `repr(tuple[()]) == "tuple[()]"`)
        // rather than the empty string that joining an empty list yields.
        let args_repr = match s.args.kind() {
            ValueKind::Tuple([]) => "()".to_string(),
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
            // `__parameters__` is the de-duplicated tuple of type variables
            // collected from `__args__` (CPython's `_Py_make_parameters`).
            // It is GenericAlias's own attribute and must not proxy to origin.
            "__parameters__" => Some(collect_parameters(&s.args)),
            _ => None,
        }
    }

    /// `list[int] == list[int]` must be `True` (CPython behaviour).
    ///
    /// Two `GenericAlias` values are equal iff their `__origin__` and
    /// `__args__` are equal.  `__origin__` uses `Value::eq` (pointer-identity
    /// for `PyClass` singletons); `__args__` is a tuple, so it recurses
    /// element-wise.  Any non-`GenericAlias` `other` compares unequal.
    ///
    /// `typing.Union` aliases are the exception: CPython compares them by
    /// `frozenset(self.__args__) == frozenset(other.__args__)`, so member
    /// order is irrelevant (`Union[int, str] == Union[str, int]`).  The
    /// flatten helper already de-dups args, so an order-insensitive
    /// element-wise comparison matches that frozenset semantics.
    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let borrow = state.borrow();
        let s = match borrow.downcast_ref::<GenericAliasState>() {
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
            let other_s = match other_borrow.downcast_ref::<GenericAliasState>() {
                Some(s) => s,
                None => return false,
            };
            if s.origin != other_s.origin {
                return false;
            }
            if is_union_origin(&s.origin) {
                return union_args_set_eq(&s.args, &other_s.args);
            }
            s.args == other_s.args
        } else {
            false
        }
    }

    /// `hash(list[int])` — CPython computes this as `hash(origin) ^ hash(args)`.
    ///
    /// `origin` is always a `PyClass` singleton; we use its `Rc` pointer as a
    /// stable integer.  `args` is a tuple of `PyClass` pointers; we hash each
    /// element pointer in turn.  This gives a hash consistent with `eq`: two
    /// aliases with the same singleton origin and equal args produce the same
    /// hash.  Returns `None` if any arg is unhashable (e.g. `list[[1,2,3]]`).
    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<GenericAliasState>()?;
        let origin_hash = value_hash_u64(&s.origin)?;
        // `typing.Union` hashes its args as a `frozenset` (order-independent),
        // so `hash(Union[int, str]) == hash(Union[str, int])` and stays
        // consistent with the order-insensitive `eq` above.  XOR of the
        // per-element hashes is commutative, matching frozenset's semantics
        // (args are already de-duplicated by the flatten helper).
        let args_hash = if is_union_origin(&s.origin) {
            union_args_set_hash(&s.args)?
        } else {
            value_hash_u64(&s.args)?
        };
        Some(origin_hash ^ args_hash)
    }

    /// `GenericAlias` values are hashable (when their args are) and can serve
    /// as dict/set keys.  Stores the shared `state` `Rc` in the `PyKey::Object`
    /// so that `Value::eq` dispatches back to our content-aware `eq` impl.
    fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
        let combined = self.hash(state)?;
        // Reconstruct a Value wrapping this same shared state so that
        // `PyKey::Object`'s `PartialEq` (`Value::eq`) dispatches back to
        // our `eq` impl and compares by content rather than by pointer.
        let value = Value::builtin_object_shared(GENERIC_ALIAS_OPS, state.clone());
        Some(PyKey::Object {
            hash: combined,
            value,
        })
    }
}

/// True if `v` is the `NoneType` class singleton (the union component that
/// `None` lowers to).  Matched by qualname so the union repr can collapse
/// `Union[X, NoneType]` to `Optional[X]`.
fn is_none_type_class(v: &Value) -> bool {
    matches!(v.kind(), ValueKind::PyClass(rc) if rc.borrow().qualname == "NoneType")
}

/// Produce the repr for a single type argument, matching how CPython formats
/// `GenericAlias.__repr__`.  For a class this is just the qualified name
/// (e.g. `"int"`, `"str"`).  For nested GenericAlias values (e.g.
/// `list[list[int]]`) it recursively produces `"list[int]"`.  For a
/// `PyInstance` that has a `__name__` attribute (e.g. a `TypeVar` created by
/// PEP 695 `type X[T] = ...` syntax), we use the `__name__` string directly
/// so that `list[T]` renders as `list[T]` rather than `list[<object at ...>]`.
/// For anything else we fall back to the general `Value::repr_raw()`.
fn repr_type_arg(v: &Value) -> String {
    match v.kind() {
        // CPython's `ga_repr_item` special-cases `Ellipsis` to render `...`
        // rather than its bare repr (`Ellipsis`), so `tuple[int, ...]` prints
        // as `tuple[int, ...]` instead of `tuple[int, Ellipsis]`.
        ValueKind::Ellipsis => "...".to_string(),
        ValueKind::PyClass(rc) => rc.borrow().qualname.clone(),
        ValueKind::BuiltinObject { ops, state } if ops.type_name() == TYPE_NAME => ops.repr(state),
        ValueKind::PyInstance(inst_rc) => {
            if let Some(name_val) = inst_rc.borrow().attrs.get("__name__")
                && let Some(s) = name_val.as_str()
            {
                return s.to_string();
            }
            v.repr_raw()
        }
        _ => v.repr_raw(),
    }
}

/// Collect the type-variable parameters from a `GenericAlias`'s `__args__`,
/// de-duplicated and in first-seen order, matching CPython's
/// `_Py_make_parameters`.  A parameter is any argument that is "type-variable
/// like" — in pyrust a `TypeVar` is a `PyInstance` carrying a `__name__`
/// attribute (see `typing.rs`).  Nested `GenericAlias` arguments (e.g.
/// `dict[str, list[T]]`) contribute their own parameters recursively.  Plain
/// classes (`int`, `str`) and `Ellipsis` are not parameters.  Always returns a
/// tuple (empty for fully-concrete aliases like `list[int]`).
fn collect_parameters(args: &Value) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let items: Vec<Value> = match args.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => vec![args.clone()],
    };
    for item in items {
        match item.kind() {
            // Nested GenericAlias: pull its parameters in.
            ValueKind::BuiltinObject { ops, state } if ops.type_name() == TYPE_NAME => {
                let borrow = state.borrow();
                if let Some(s) = borrow.downcast_ref::<GenericAliasState>()
                    && let ValueKind::Tuple(nested) = collect_parameters(&s.args).kind()
                {
                    for p in nested.iter() {
                        push_unique(&mut out, p.clone());
                    }
                }
            }
            // A TypeVar is a PyInstance with a `__name__` attribute.
            ValueKind::PyInstance(inst_rc) if inst_rc.borrow().attrs.get("__name__").is_some() => {
                push_unique(&mut out, item.clone());
            }
            _ => {}
        }
    }
    Value::tuple(out)
}

/// Append `v` to `out` only if no element already equal to it is present
/// (preserving first-seen order), matching CPython's tuple-dedup in
/// `_Py_make_parameters`.
fn push_unique(out: &mut Vec<Value>, v: Value) {
    if !out.contains(&v) {
        out.push(v);
    }
}

/// Compute a `u64` hash for a `Value` for use in `GenericAlias` key
/// construction.
///
/// Uses `Value::to_key` for types that have a natural `PyKey` (int, str,
/// tuple, frozenset, …), falling back to Rc pointer identity for `PyClass`
/// singletons (which have no `PyKey` because they are not hashable at the
/// Python level under the old design — but `type` objects *are* hashable in
/// CPython and are always singletons in pyrust, so pointer hash is stable and
/// consistent with identity-based `Value::eq`).
///
/// Returns `None` if the value is genuinely unhashable (e.g. a list).
fn value_hash_u64(v: &Value) -> Option<u64> {
    if let Some(key) = v.to_key() {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        return Some(h.finish());
    }
    // PyClass: use the Rc pointer address as a stable hash.  Two `PyClass`
    // values with the same address are the same singleton (pointer equality is
    // the `Value::eq` definition for PyClass, line 2810 of pyrust-core).
    if let ValueKind::PyClass(rc) = v.kind() {
        let ptr = std::rc::Rc::as_ptr(rc) as u64;
        let mut h = DefaultHasher::new();
        ptr.hash(&mut h);
        return Some(h.finish());
    }
    // Tuple of PyClass pointers (the args tuple).
    if let ValueKind::Tuple(items) = v.kind() {
        let mut h = DefaultHasher::new();
        items.len().hash(&mut h);
        for item in items.iter() {
            value_hash_u64(item)?.hash(&mut h);
        }
        return Some(h.finish());
    }
    None
}

/// If `v` is a `GenericAlias`, return a clone of its `__origin__`.
///
/// Used by the interpreter's call path (issue #2133): calling a `GenericAlias`
/// (`list[int](x)`) delegates to the origin (`list(x)`), which requires
/// interpreter access to run the origin's constructor — so the interpreter
/// asks for the origin here and re-dispatches the call itself.
pub fn as_generic_alias_origin(v: &Value) -> Option<Value> {
    if let ValueKind::BuiltinObject { ops, state } = v.kind()
        && ops.type_name() == TYPE_NAME
    {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<GenericAliasState>()?;
        return Some(s.origin.clone());
    }
    None
}

/// Read a `GenericAlias`'s `(origin, args)` pair, if `v` is one.
///
/// Used by the `typing` module's `Union`/`Optional` flatten helper, which
/// needs to splice a nested alias's `__args__` into the outer union.
pub fn as_generic_alias_origin_args(v: &Value) -> Option<(Value, Value)> {
    if let ValueKind::BuiltinObject { ops, state } = v.kind()
        && ops.type_name() == TYPE_NAME
    {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<GenericAliasState>()?;
        return Some((s.origin.clone(), s.args.clone()));
    }
    None
}

/// True if `origin` is the `typing.Union` special-form class — the origin
/// every flattened `Union`/`Optional` alias carries.  Matched by
/// `__qualname__ == "Union"` plus `__module__ == "typing"` so it does not also
/// catch a user class happening to be named `Union`.
fn is_union_origin(origin: &Value) -> bool {
    if let ValueKind::PyClass(rc) = origin.kind() {
        let c = rc.borrow();
        return c.qualname == "Union"
            && c.attrs.get("__module__").and_then(|m| m.as_str()) == Some("typing");
    }
    false
}

/// Order-insensitive equality of two `Union` arg tuples, mirroring CPython's
/// `frozenset(a.__args__) == frozenset(b.__args__)`.  The flatten helper
/// de-dups args, so equal length plus every member of `a` present in `b` is a
/// faithful set comparison.
fn union_args_set_eq(a: &Value, b: &Value) -> bool {
    match (a.kind(), b.kind()) {
        (ValueKind::Tuple(xs), ValueKind::Tuple(ys)) => {
            xs.len() == ys.len() && xs.iter().all(|x| ys.iter().any(|y| x == y))
        }
        _ => a == b,
    }
}

/// Order-independent hash of a `Union` arg tuple, consistent with
/// `union_args_set_eq`.  XOR of the per-element hashes is commutative, matching
/// the `frozenset(args)` hash CPython uses.  Returns `None` if any arg is
/// unhashable.
fn union_args_set_hash(args: &Value) -> Option<u64> {
    match args.kind() {
        ValueKind::Tuple(items) => {
            let mut acc: u64 = 0;
            for item in items.iter() {
                acc ^= value_hash_u64(item)?;
            }
            Some(acc)
        }
        _ => value_hash_u64(args),
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
