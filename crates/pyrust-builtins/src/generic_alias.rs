//! `GenericAlias` value — returned by `list[int]`, `dict[str, int]`, etc.
//!
//! PEP 585 (Python 3.9+) lets you write `list[int]` instead of
//! `typing.List[int]`.  Built-in collection types expose `__class_getitem__`
//! as a classmethod; subscripting them creates a `types.GenericAlias` that
//! carries `(__origin__, __args__)` and has a human-readable repr.
//!
//! This module provides the pyrust equivalent as a `BuiltinTypeOps`
//! implementation backed by `GenericAliasState`.

use std::any::Any;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, CanonicalClassTag, PyClass, PyKey, Value, ValueKind,
    builtin_ops_is,
};

pub struct GenericAliasState {
    /// The origin type (e.g. the `list` class value).
    pub origin: Value,
    /// The type argument(s).  For a single-arg subscript (`list[int]`) this is
    /// a one-element tuple; for multi-arg (`dict[str, int]`) it is a two-(or
    /// more)-element tuple.  Matches CPython's `GenericAlias.__args__`.
    pub args: Value,
}

/// Operations table for one GenericAlias semantic family.
///
/// `typing.Union` needs order-independent equality and hashing, while ordinary
/// PEP 585 aliases are ordered. Encoding that distinction in the zero-sized
/// operations type keeps the per-alias state to its two visible `Value`s.
pub struct GenericAliasOps<const TYPING_UNION: bool>;

pub const GENERIC_ALIAS_OPS: &GenericAliasOps<false> = &GenericAliasOps;
const TYPING_UNION_ALIAS_OPS: &GenericAliasOps<true> = &GenericAliasOps;
pub const TYPE_NAME: &str = "types.GenericAlias";

#[inline(always)]
fn generic_alias_ops_kind(ops: &dyn BuiltinTypeOps) -> Option<bool> {
    if builtin_ops_is::<GenericAliasOps<false>>(ops) {
        Some(false)
    } else if builtin_ops_is::<GenericAliasOps<true>>(ops) {
        Some(true)
    } else {
        None
    }
}

#[inline(always)]
fn is_generic_alias_ops(ops: &dyn BuiltinTypeOps) -> bool {
    generic_alias_ops_kind(ops).is_some()
}

impl<const TYPING_UNION: bool> BuiltinTypeOps for GenericAliasOps<TYPING_UNION> {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let (origin, args) = {
            let borrow = state.borrow();
            let alias = borrow
                .downcast_ref::<GenericAliasState>()
                .expect("GenericAliasOps: bad state");
            (alias.origin.clone(), alias.args.clone())
        };
        render_generic_alias_parts(
            &origin,
            &args,
            TYPING_UNION,
            &mut (),
            data_only_arg_repr,
            data_only_module_str,
        )
        .expect("the data-only GenericAlias renderer is infallible")
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

    fn has_method(&self, name: &str) -> bool {
        name == "__mro_entries__"
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> pyrust_core::Result<Value> {
        if name != "__mro_entries__" {
            return Err(pyrust_core::PyError::attribute_error(
                format!("'types.GenericAlias' object has no attribute '{name}'"),
                Some(name.to_string()),
                None,
            ));
        }
        if !kwargs.is_empty() {
            return Err(pyrust_core::type_err!(
                "GenericAlias.__mro_entries__() takes no keyword arguments"
            ));
        }
        if args.len() != 1 {
            return Err(pyrust_core::type_err!(
                "GenericAlias.__mro_entries__() takes exactly one argument ({} given)",
                args.len()
            ));
        }
        let borrow = state.borrow();
        let alias = borrow
            .downcast_ref::<GenericAliasState>()
            .ok_or_else(|| pyrust_core::PyError::Runtime("invalid GenericAlias state".into()))?;
        Ok(Value::tuple(vec![alias.origin.clone()]))
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
            if generic_alias_ops_kind(other_ops) != Some(TYPING_UNION) {
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
            if TYPING_UNION {
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
        let args_hash = if TYPING_UNION {
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
        let ops: &'static dyn BuiltinTypeOps = if TYPING_UNION {
            TYPING_UNION_ALIAS_OPS
        } else {
            GENERIC_ALIAS_OPS
        };
        let value = Value::builtin_object_shared(ops, state.clone());
        Some(PyKey::Object {
            hash: combined,
            value,
        })
    }
}

/// Render a GenericAlias while delegating ordinary argument reprs to the
/// interpreter. The alias state is snapshotted before the callback runs, so a
/// user `__repr__` may safely re-enter the same alias.
pub fn render_generic_alias_with<C>(
    value: &Value,
    context: &mut C,
    render_other: fn(&mut C, &Value) -> pyrust_core::Result<String>,
    render_module_str: fn(&mut C, &Value) -> pyrust_core::Result<String>,
) -> pyrust_core::Result<String> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return Err(pyrust_core::PyError::Runtime(
            "expected GenericAlias value".to_string(),
        ));
    };
    let Some(typing_union) = generic_alias_ops_kind(ops) else {
        return Err(pyrust_core::PyError::Runtime(
            "expected GenericAlias value".to_string(),
        ));
    };
    let (origin, args) = {
        let borrow = state.borrow();
        let alias = borrow
            .downcast_ref::<GenericAliasState>()
            .ok_or_else(|| pyrust_core::PyError::Runtime("invalid GenericAlias state".into()))?;
        (alias.origin.clone(), alias.args.clone())
    };
    render_generic_alias_parts(
        &origin,
        &args,
        typing_union,
        context,
        render_other,
        render_module_str,
    )
}

fn render_generic_alias_parts<C>(
    origin: &Value,
    args: &Value,
    typing_union: bool,
    context: &mut C,
    render_other: fn(&mut C, &Value) -> pyrust_core::Result<String>,
    render_module_str: fn(&mut C, &Value) -> pyrust_core::Result<String>,
) -> pyrust_core::Result<String> {
    // CPython applies the same class-qualifier spelling to a GenericAlias's
    // origin as to a PEP 585 class argument. The shared helper snapshots raw
    // class metadata before a non-string module invokes Python-visible str().
    let origin_name = match origin.kind() {
        ValueKind::PyClass(class) => repr_class_type_arg_with(
            class,
            ClassTypeArgReprStyle::Pep585,
            context,
            render_module_str,
        )?,
        // A PEP 695 `TypeAliasType` origin (`type Pair[T] = ...; Pair[int]`)
        // is a `PyInstance` carrying a `__name__` string; CPython reprs the
        // parameterized alias as `Pair[int]` using that name (issue #2779).
        ValueKind::PyInstance(rc) => rc
            .borrow()
            .attrs
            .get("__name__")
            .and_then(|n| n.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| pyrust_core::builtin_type_name(origin).into_owned()),
        _ => pyrust_core::builtin_type_name(origin).into_owned(),
    };

    // `typing.Union[X, NoneType]` (exactly two args, one of them
    // `NoneType`) renders as `typing.Optional[X]`, mirroring CPython's
    // `_SpecialForm`/`_GenericAlias.__repr__` for unions.  The flatten
    // helper in `typing.rs` always lowers `Optional[...]` to a `Union`
    // origin, so this is the single place the `Optional` spelling is
    // reconstructed.
    if typing_union
        && let ValueKind::Tuple(items) = args.kind()
        && items.len() == 2
    {
        let none_pos = items.iter().position(is_none_type_class);
        if let Some(pos) = none_pos {
            let other = &items[1 - pos];
            return Ok(format!(
                "typing.Optional[{}]",
                repr_type_arg(
                    other,
                    false,
                    ClassTypeArgReprStyle::Typing,
                    context,
                    render_other,
                    render_module_str,
                )?
            ));
        }
    }

    let lower_none = origin_name.starts_with("typing.") && origin_name != "typing.Literal";
    let class_style = if origin_name.starts_with("typing.") {
        ClassTypeArgReprStyle::Typing
    } else {
        ClassTypeArgReprStyle::Pep585
    };

    let args_repr = match args.kind() {
        ValueKind::Tuple([]) => "()".to_string(),
        ValueKind::Tuple(items) => items
            .iter()
            .map(|arg| {
                repr_type_arg(
                    arg,
                    lower_none,
                    class_style,
                    context,
                    render_other,
                    render_module_str,
                )
            })
            .collect::<pyrust_core::Result<Vec<_>>>()?
            .join(", "),
        _ => repr_type_arg(
            args,
            lower_none,
            class_style,
            context,
            render_other,
            render_module_str,
        )?,
    };

    Ok(format!("{origin_name}[{args_repr}]"))
}

fn data_only_arg_repr(_context: &mut (), value: &Value) -> pyrust_core::Result<String> {
    if let ValueKind::PyInstance(instance) = value.kind()
        && let Some(name) = instance.borrow().attrs.get("__name__")
        && let Some(name) = name.as_str()
    {
        return Ok(name.to_string());
    }
    Ok(value.repr_raw())
}

fn data_only_module_str(_context: &mut (), value: &Value) -> pyrust_core::Result<String> {
    Ok(value.to_py_str())
}

/// True if `v` is the `NoneType` class singleton (the union component that
/// `None` lowers to). The immutable canonical tag prevents a user class named
/// `NoneType` from being mistaken for the runtime singleton.
fn is_none_type_class(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::PyClass(class)
            if class.borrow().canonical_tag == Some(CanonicalClassTag::NoneType)
    )
}

/// Produce the repr for a single type argument, matching how CPython formats
/// `GenericAlias.__repr__`.  For a class this is just the qualified name
/// (e.g. `"int"`, `"str"`).  For nested GenericAlias values (e.g.
/// `list[list[int]]`) it recursively produces `"list[int]"`.  For a
/// A canonical `TypeVar` uses its immutable variance metadata. Everything
/// else delegates to either interpreter-aware repr dispatch or the data-only
/// fallback selected by the caller.
fn repr_type_arg<C>(
    v: &Value,
    lower_none: bool,
    class_style: ClassTypeArgReprStyle,
    context: &mut C,
    render_other: fn(&mut C, &Value) -> pyrust_core::Result<String>,
    render_module_str: fn(&mut C, &Value) -> pyrust_core::Result<String>,
) -> pyrust_core::Result<String> {
    match v.kind() {
        // CPython's `ga_repr_item` special-cases `Ellipsis` to render `...`
        // rather than its bare repr (`Ellipsis`), so `tuple[int, ...]` prints
        // as `tuple[int, ...]` instead of `tuple[int, Ellipsis]`.
        ValueKind::Ellipsis => Ok("...".to_string()),
        // A bare `None` renders as `None` for PEP 585 builtin aliases
        // (`list[None]`) and `typing.Literal`, but lowers to `NoneType` for
        // every other `typing.*` special form, which substitutes `type(None)`
        // at construction (`typing.Final[None]` → `typing.Final[NoneType]`,
        // `Callable[[], None]` → `typing.Callable[[], NoneType]`).  The caller
        // sets `lower_none` accordingly (see the `lower_none` derivation in
        // `repr`); it carries that context down here and into Callable's
        // parameter list.
        ValueKind::None if lower_none => Ok("NoneType".to_string()),
        ValueKind::None => Ok("None".to_string()),
        ValueKind::PyClass(class) => {
            repr_class_type_arg_with(class, class_style, context, render_module_str)
        }
        // The parameter-list of a `Callable[[int, str], ret]` subscript is
        // stored as a `list` argument.  Render it as `[int, str]`, recursing
        // so each element uses its type repr (`int`, not `<class 'int'>`).  The
        // `lower_none` context propagates into the param list (Callable lowers
        // `None` in its parameters too) but not into nested aliases below.
        ValueKind::List(items) => {
            let inner = items
                .iter()
                .map(|arg| {
                    repr_type_arg(
                        arg,
                        lower_none,
                        class_style,
                        context,
                        render_other,
                        render_module_str,
                    )
                })
                .collect::<pyrust_core::Result<Vec<_>>>()?
                .join(", ");
            Ok(format!("[{inner}]"))
        }
        ValueKind::PyInstance(_) => {
            if let Some(rendered) = typevar_arg_repr(v) {
                return Ok(rendered);
            }
            render_other(context, v)
        }
        _ => render_other(context, v),
    }
}

/// Render a TypeVar argument without interpreter dispatch. TypeVar variance
/// attributes are immutable booleans, so this preserves `~T` / `+T` / `-T`
/// while never invoking an arbitrary user `__repr__` implementation.
pub fn typevar_arg_repr(value: &Value) -> Option<String> {
    let ValueKind::PyInstance(instance) = value.kind() else {
        return None;
    };
    let instance = instance.borrow();
    if instance.class.borrow().canonical_tag != Some(CanonicalClassTag::TypeVar) {
        return None;
    }
    let name = instance.attrs.get("__name__")?.as_str()?;
    let flag = |attr: &str| {
        instance
            .attrs
            .get(attr)
            .is_some_and(|value| matches!(value.kind(), ValueKind::Bool(true)))
    };
    let prefix = if flag("__infer_variance__") {
        ""
    } else if flag("__covariant__") {
        "+"
    } else if flag("__contravariant__") {
        "-"
    } else {
        "~"
    };
    Some(format!("{prefix}{name}"))
}

/// Which CPython class-argument spelling owns a generic alias repr.
#[derive(Clone, Copy)]
pub enum ClassTypeArgReprStyle {
    /// `types.GenericAlias` (`list[C]`): a `None` module falls back to the
    /// class repr, while every other module value qualifies `__qualname__`.
    Pep585,
    /// `typing` aliases: every module value, including `None`, qualifies the
    /// class `__qualname__`.
    Typing,
}

/// Render a class used as a generic-alias argument.
///
/// The class metadata is snapshotted before a non-string `__module__` is
/// delegated to Python-visible `str()` dispatch. This keeps user code outside
/// the class borrow while preserving the data-only path used by `repr_raw()`.
pub fn repr_class_type_arg_with<C>(
    class: &Rc<RefCell<PyClass>>,
    style: ClassTypeArgReprStyle,
    context: &mut C,
    render_module_str: fn(&mut C, &Value) -> pyrust_core::Result<String>,
) -> pyrust_core::Result<String> {
    let (module, name, qualname) = {
        let class = class.borrow();
        (
            class.attrs.get("__module__").cloned(),
            class.name.clone(),
            class.qualname.clone(),
        )
    };
    let Some(module) = module else {
        // Canonical builtin classes synthesize their `builtins` module at the
        // attribute layer rather than storing it in the raw class mapping.
        return Ok(qualname);
    };
    if matches!(style, ClassTypeArgReprStyle::Pep585) && module.is_none() {
        return Ok(format!("<class '{name}'>"));
    }
    let module = match raw_string_text(&module) {
        Some(module) => module,
        None => render_module_str(context, &module)?,
    };
    if module == "builtins" {
        Ok(qualname)
    } else {
        Ok(format!("{module}.{qualname}"))
    }
}

fn raw_string_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    let ValueKind::PyInstance(instance) = value.kind() else {
        return None;
    };
    let instance = instance.borrow();
    if !class_chain_is_str(&instance.class) {
        return None;
    }
    instance
        .attrs
        .get("__builtin_data__")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn class_chain_is_str(class: &Rc<RefCell<PyClass>>) -> bool {
    let class = class.borrow();
    class.canonical_tag == Some(CanonicalClassTag::Str)
        || class.base.as_ref().is_some_and(class_chain_is_str)
        || class.extra_bases.iter().any(class_chain_is_str)
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
            ValueKind::BuiltinObject { ops, state } if is_generic_alias_ops(ops) => {
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

/// Return whether `v` is a `types.GenericAlias` value owned by this module.
///
/// The concrete Rust operations type is stable even though `type_name()` is
/// presentation metadata.
#[inline]
pub fn is_generic_alias(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::BuiltinObject { ops, .. } if is_generic_alias_ops(ops)
    )
}

/// If `v` is a `GenericAlias`, return a clone of its `__origin__`.
///
/// Used by the interpreter's call path (issue #2133): calling a `GenericAlias`
/// (`list[int](x)`) delegates to the origin (`list(x)`), which requires
/// interpreter access to run the origin's constructor — so the interpreter
/// asks for the origin here and re-dispatches the call itself.
pub fn as_generic_alias_origin(v: &Value) -> Option<Value> {
    if let ValueKind::BuiltinObject { ops, state } = v.kind()
        && is_generic_alias_ops(ops)
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
        && is_generic_alias_ops(ops)
    {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<GenericAliasState>()?;
        return Some((s.origin.clone(), s.args.clone()));
    }
    None
}

/// Read the `__args__` tuple of a `typing.Union[...]` alias, if `v` is one.
///
/// Returns `Some(args_tuple)` only when `v` is a `GenericAlias` whose origin is
/// the `typing.Union` special-form class (`get_origin(v) is Union`).  Used by
/// the `isinstance`/`issubclass` builtins to treat `typing.Union[int, str]`
/// like the tuple `(int, str)`, matching CPython 3.12.
pub fn as_typing_union_args(v: &Value) -> Option<Value> {
    if let ValueKind::BuiltinObject { ops, state } = v.kind()
        && builtin_ops_is::<GenericAliasOps<true>>(ops)
    {
        let borrow = state.borrow();
        let alias = borrow.downcast_ref::<GenericAliasState>()?;
        return Some(alias.args.clone());
    }
    None
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

/// Construct the internal alias representation owned by
/// `typing.Union`/`typing.Optional`.
///
/// The typing module has already resolved the canonical Union class for its
/// active generation, so it selects Union equality/hash semantics explicitly.
/// Keeping this separate from [`generic_alias`] avoids stdlib identity lookups
/// on every ordinary PEP 585 alias construction.
pub fn typing_union_alias(origin: Value, args: Value) -> Value {
    let state: Box<dyn Any> = Box::new(GenericAliasState { origin, args });
    Value::builtin_object(TYPING_UNION_ALIAS_OPS, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyrust_core::PyClass;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn test_class(name: &str) -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass::new(
            name,
            name,
            None,
            IndexMap::new(),
        )))
    }

    #[test]
    fn generic_alias_state_remains_two_nanboxed_values() {
        assert_eq!(
            std::mem::size_of::<GenericAliasState>(),
            2 * std::mem::size_of::<Value>()
        );
    }

    #[test]
    fn only_typing_owned_union_aliases_use_order_independent_semantics() {
        let origin = Value::py_class(test_class("Union"));
        let forward_args = Value::tuple(vec![
            Value::py_class(test_class("int")),
            Value::py_class(test_class("str")),
        ]);
        let ValueKind::Tuple(forward_items) = forward_args.kind() else {
            unreachable!("constructed tuple");
        };
        let reverse_args = Value::tuple(vec![forward_items[1].clone(), forward_items[0].clone()]);

        let direct = generic_alias(origin.clone(), forward_args.clone());
        let direct_reversed = generic_alias(origin.clone(), reverse_args.clone());
        assert!(direct != direct_reversed);
        assert!(as_typing_union_args(&direct).is_none());

        let typing = typing_union_alias(origin.clone(), forward_args);
        let typing_reversed = typing_union_alias(origin, reverse_args.clone());
        assert!(typing == typing_reversed);
        assert_eq!(as_typing_union_args(&typing_reversed), Some(reverse_args));
    }
}
