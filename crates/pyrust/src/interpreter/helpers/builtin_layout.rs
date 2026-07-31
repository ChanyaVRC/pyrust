/// `isinstance(v, (str, bytes, bytearray, dict, set, frozenset))` — the set of
/// types PEP 634 §3 excludes from sequence-pattern matching.
///
/// Replicates [`isinstance_single`]'s semantics for those primitive classes
/// (including subclass instances) without building a tuple of type objects or
/// going through the generic `isinstance` call.  Backs the `MatchSeqExcluded`
/// instruction so a `match` arm with a sequence pattern pays one
/// allocation-free check per execution instead of rebuilding the exclusion
/// tuple every time (issue #1789).
///
/// `bytearray` is excluded (issue #1844): although it is a mutable byte buffer,
/// CPython does not set `Py_TPFLAGS_SEQUENCE` on it, so `match bytearray(b"ab")`
/// against `case [a, b]` is a no-match.
pub(crate) fn value_is_seq_excluded(v: &Value) -> bool {
    match v.kind() {
        // Direct primitives — the common case, decided by the NaN-box tag.
        ValueKind::Str(_) | ValueKind::Bytes(_) | ValueKind::Dict(_) | ValueKind::Set(_) => true,
        ValueKind::BuiltinObject { ops, .. } => matches!(
            ops.canonical_class_tag(),
            Some(
                pyrust_core::CanonicalClassTag::Bytearray
                    | pyrust_core::CanonicalClassTag::Frozenset
            )
        ),
        // Subclass instances (`class MyDict(dict)`): walk the MRO against each
        // excluded primitive singleton, matching `isinstance(_, dict)` etc.
        ValueKind::PyInstance(inst) => {
            let actual = Rc::clone(&inst.borrow().class);
            PRIMITIVE_CLASSES.with(|c| {
                class_is_subclass_of(&actual, &c.str_class)
                    || class_is_subclass_of(&actual, &c.bytes_class)
                    || class_is_subclass_of(&actual, &c.bytearray_class)
                    || class_is_subclass_of(&actual, &c.dict_class)
                    || class_is_subclass_of(&actual, &c.set_class)
                    || class_is_subclass_of(&actual, &c.frozenset_class)
            })
        }
        _ => false,
    }
}

/// True if `v` is a mapping for the purposes of `match` mapping patterns
/// (`case {k: p}`).  PEP 634 §3 gates the whole mapping pattern on
/// `isinstance(subject, collections.abc.Mapping)`; a non-mapping subject
/// silently fails to match instead of raising. The owning `collections.abc`
/// module registers its Mapping class by weak identity. This keeps the VM
/// independent of that module while preserving explicit Mapping subclasses
/// and renamed class objects.
pub(crate) fn value_is_mapping(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Dict(_) => true,
        // `mappingproxy` (e.g. `type(C).__dict__`) is registered as a
        // `collections.abc.Mapping` in CPython, so it matches a mapping
        // pattern (issue #1879). Its owner exposes the concrete Rust identity
        // without leaking the Python presentation name into this layer.
        ValueKind::BuiltinObject { .. } => pyrust_builtins::mapping_proxy::is_mapping_proxy(v),
        // Subclass instances: accept the primitive dict hierarchy and any
        // explicit collections.abc.Mapping hierarchy registered by its owner.
        ValueKind::PyInstance(inst) => {
            let actual = Rc::clone(&inst.borrow().class);
            PRIMITIVE_CLASSES.with(|c| class_is_subclass_of(&actual, &c.dict_class))
                || pyrust_builtins::abc_marker::any_live_class(
                    pyrust_builtins::abc_marker::AbcKind::Mapping,
                    |mapping| class_is_subclass_of(&actual, mapping),
                )
        }
        _ => false,
    }
}

/// Returns the type name if `class` is one of the builtin types that
/// CPython marks as non-subclassable (i.e. lacks `Py_TPFLAGS_BASETYPE`):
/// `NoneType`, `ellipsis`, `NotImplementedType`, `bool`, `method`,
/// `function`, and the final typing runtime types.
///
/// Used in the `MakeClass` instruction to raise `TypeError: type 'X' is
/// not an acceptable base type` before the class body runs.
pub(crate) fn non_subclassable_builtin_name(class: &Rc<RefCell<PyClass>>) -> Option<&'static str> {
    if let Some(name) = class.borrow().non_subclassable_name {
        return Some(name);
    }
    let ptr = Rc::as_ptr(class);
    // Check the METHOD_TYPE and FUNCTION_TYPE singletons (issue #1528): CPython
    // raises `TypeError: type 'method'/'function' is not an acceptable base type`
    // when either is used as a base class.
    if METHOD_TYPE.with(|m| ptr == Rc::as_ptr(m)) {
        return Some("method");
    }
    if FUNCTION_TYPE.with(|f| ptr == Rc::as_ptr(f)) {
        return Some("function");
    }
    // Issues #1793, #1800: RANGE_CLASS is a proper PyClass singleton so that
    // `issubclass(range, Sequence)` works, but `range` is not subclassable in
    // CPython (`TypeError: type 'range' is not an acceptable base type`).
    if RANGE_CLASS.with(|r| ptr == Rc::as_ptr(r)) {
        return Some("range");
    }
    PRIMITIVE_CLASSES.with(|c| {
        if ptr == Rc::as_ptr(&c.none_class) {
            return Some("NoneType");
        }
        if ptr == Rc::as_ptr(&c.notimplemented_class) {
            return Some("NotImplementedType");
        }
        if ptr == Rc::as_ptr(&c.ellipsis_class) {
            return Some("ellipsis");
        }
        if ptr == Rc::as_ptr(&c.bool_class) {
            return Some("bool");
        }
        None
    })
}

/// Returns `true` if `val` is a `tuple` or an instance of a `tuple` subclass
/// (e.g. a `namedtuple` such as `time.struct_time`).  Mirrors CPython's
/// `PyTuple_Check`, which several stdlib functions (`time.mktime` /
/// `time.strftime`) use to reject non-tuple sequences like `list` / `str`.
pub(crate) fn is_tuple_or_tuple_subclass(val: &Value) -> bool {
    match val.kind() {
        ValueKind::Tuple(_) => true,
        ValueKind::PyInstance(inst) => {
            PRIMITIVE_CLASSES.with(|c| class_is_subclass_of(&inst.borrow().class, &c.tuple_class))
        }
        _ => false,
    }
}

/// Returns `true` if `class` is one of the built-in types that carry a
/// non-trivial C-level instance layout (`int`, `str`, `float`, `bytes`,
/// `tuple`, `list`, `dict`, `set`, `frozenset`).  CPython raises
/// `TypeError: multiple bases have instance lay-out conflict` when two or
/// more such types appear in the same bases tuple.  Issue #1677.
pub(crate) fn is_solid_primitive_class(class: &Rc<RefCell<PyClass>>) -> bool {
    let ptr = Rc::as_ptr(class);
    PRIMITIVE_CLASSES.with(|c| {
        ptr == Rc::as_ptr(&c.int_class)
            || ptr == Rc::as_ptr(&c.str_class)
            || ptr == Rc::as_ptr(&c.float_class)
            || ptr == Rc::as_ptr(&c.bytes_class)
            || ptr == Rc::as_ptr(&c.tuple_class)
            || ptr == Rc::as_ptr(&c.list_class)
            || ptr == Rc::as_ptr(&c.dict_class)
            || ptr == Rc::as_ptr(&c.set_class)
            || ptr == Rc::as_ptr(&c.frozenset_class)
    })
}

/// Returns `true` if `class` introduces extra instance variables beyond its
/// base — i.e. it has a non-empty `__slots__` (counting only real member
/// slots, not the `__dict__` / `__weakref__` sentinels, which only toggle the
/// dict/weakref layout and are not counted by CPython's `extra_ivars`), or it
/// is a built-in primitive with its own C-level layout.  Mirrors the part of
/// CPython's `extra_ivars` that distinguishes a fresh "solid base".
fn class_adds_ivars(class: &Rc<RefCell<PyClass>>) -> bool {
    if is_solid_primitive_class(class) {
        return true;
    }
    let borrowed = class.borrow();
    borrowed.slots.as_ref().is_some_and(|s| {
        s.iter()
            .any(|name| name != "__dict__" && name != "__weakref__")
    })
}

/// CPython's `solid_base(type)`: the most-derived ancestor of `class` whose
/// instance layout differs from `object`'s.  A class shares its base's solid
/// base unless it adds real instance variables (non-empty `__slots__` or a
/// primitive C layout); otherwise the solid base is inherited.  Returns `None`
/// to mean the `object` layout (no solid base).
pub(crate) fn solid_base(class: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    if class_adds_ivars(class) {
        return Some(Rc::clone(class));
    }
    let base = class.borrow().base.clone();
    base.and_then(|b| solid_base(&b))
}

/// CPython's `best_base` layout-conflict guard: given the resolved list of
/// direct bases, returns `true` when two of them have incompatible instance
/// layouts (neither base's solid base is a subtype of the other's).  Raising
/// `TypeError: multiple bases have instance lay-out conflict` in that case
/// matches CPython for both the C-level case (`int` + `str`, issue #1677) and
/// the user-`__slots__` case (issue #2109).
pub(crate) fn bases_have_layout_conflict(bases: &[Rc<RefCell<PyClass>>]) -> bool {
    // Track the current "winner" solid base; each subsequent base must be in a
    // subtype relationship with it (one a subtype of the other), else conflict.
    let mut winner: Option<Rc<RefCell<PyClass>>> = None;
    for base in bases {
        let Some(candidate) = solid_base(base) else {
            continue;
        };
        match &winner {
            None => winner = Some(candidate),
            Some(current) => {
                if class_is_subclass_of(&candidate, current) {
                    // candidate is more derived — it becomes the winner.
                    winner = Some(candidate);
                } else if !class_is_subclass_of(current, &candidate) {
                    // Neither is a subtype of the other: incompatible layouts.
                    return true;
                }
                // else: current already subsumes candidate; keep winner.
            }
        }
    }
    false
}

/// Walk the base chain of `class` and return the name of the first
/// primitive builtin base found (`"dict"`, `"list"`, `"set"`, …), or
/// `None` if the class does not inherit from any primitive.
///
/// Only the directly-supported container primitives that need backing-
/// data storage are returned: `dict`, `list`, and `set`.  Other
/// primitives (`int`, `str`, `float`, …) require deep storage-variant
/// changes and are out of scope for issue #976.
pub(crate) fn find_mutable_primitive_base(class: &Rc<RefCell<PyClass>>) -> Option<&'static str> {
    if let Some(
        kind @ (PrimitiveClassKind::Dict | PrimitiveClassKind::List | PrimitiveClassKind::Set),
    ) = primitive_class_kind(class)
    {
        return Some(kind.canonical_name());
    }
    let base = class.borrow().base.clone();
    base.and_then(|b| find_mutable_primitive_base(&b))
}

/// Walk the base chain of `class` and return the name of the first
/// immutable primitive builtin base found (`"frozenset"` or `"tuple"`),
/// or `None` if the class does not inherit from either.
///
/// These types are immutable — their backing must be populated from the
/// constructor argument at `__new__` time, before any `__init__` runs.
/// Unlike the mutable types handled by `find_mutable_primitive_base`,
/// there is no empty pre-initialisation step (issue #994).
pub(crate) fn find_immutable_primitive_base(class: &Rc<RefCell<PyClass>>) -> Option<&'static str> {
    if let Some(kind @ (PrimitiveClassKind::Frozenset | PrimitiveClassKind::Tuple)) =
        primitive_class_kind(class)
    {
        return Some(kind.canonical_name());
    }
    let base = class.borrow().base.clone();
    base.and_then(|b| find_immutable_primitive_base(&b))
}

/// Walk the base chain of `class` and return the name of the first scalar
/// (non-container) primitive builtin base found (`"str"`, `"int"`, `"float"`,
/// `"bytes"`, or `"complex"`), or `None` if the class does not inherit from
/// any of these.
///
/// Issue #1204: these types require the same `__builtin_data__` backing-store
/// approach used by the container primitives (`dict`/`list`/`set`), so that
/// subclass instances can delegate method dispatch to the underlying primitive
/// value.  Like `find_immutable_primitive_base`, the backing is populated at
/// construction time from the constructor args and is fixed thereafter.
pub(crate) fn find_scalar_primitive_base(class: &Rc<RefCell<PyClass>>) -> Option<&'static str> {
    if let Some(
        kind @ (PrimitiveClassKind::Str
        | PrimitiveClassKind::Int
        | PrimitiveClassKind::Float
        | PrimitiveClassKind::Bytes
        | PrimitiveClassKind::Bytearray
        | PrimitiveClassKind::Complex),
    ) = primitive_class_kind(class)
    {
        return Some(kind.canonical_name());
    }
    let base = class.borrow().base.clone();
    base.and_then(|b| find_scalar_primitive_base(&b))
}

/// Constant key used to store the backing primitive value inside a
/// `PyInstance` that subclasses `dict`, `list`, or `set`.
pub(crate) const BUILTIN_DATA_ATTR: &str = "__builtin_data__";

/// Extract the backing primitive value from a `PyInstance` that was
/// constructed by `call_class_expanded` for a subclass of `dict`,
/// `list`, or `set`.  Returns `None` for any other instance.
pub(crate) fn instance_builtin_data(inst: &Rc<RefCell<PyInstance>>) -> Option<Value> {
    inst.borrow().attrs.get(BUILTIN_DATA_ATTR).cloned()
}

/// The dunder-free core of [`effective_builtin_receiver`]: the backing builtin
/// value of a builtin-subclass `PyInstance`, or `None` for any other value.
///
/// This is the version hot consumers (binary-op operand coercion, iteration,
/// dict-merge / set-op extraction) want — they perform no override check, so
/// they must not carry the override-gate tail (`lookup_class_attr` loop,
/// `Rc::clone`) of [`effective_builtin_receiver`] into their inlined body.
/// Keeping it a separate `#[inline]` fn lets the hot path collapse to the same
/// two tag/attr fetches the open-coded sites used before #2386, so coercing a
/// `class MyInt(int)` operand in a tight binary-op loop stays free of dead
/// branches. Override-aware consumers must keep calling
/// [`effective_builtin_receiver`] with their relevant dunder(s).
#[inline]
pub(crate) fn builtin_data_backing(v: &Value) -> Option<Value> {
    let inst_rc = v.as_py_instance_rc()?;
    instance_builtin_data(inst_rc)
}

/// The single representation-substitutability boundary (issue #2386).
///
/// CPython gives builtin subclasses *structural* substitutability: a
/// `class BA(bytearray)` instance physically embeds a `bytearray`, so every
/// consumer (repr, iteration, operators, method dispatch, …) works on it via
/// the inherited slot without ever asking "is this a subclass?".  pyrust
/// instead stores the base value in a `__builtin_data__` attr on a
/// `PyInstance`, a different `ValueKind` shape — so each consumer must unwrap
/// to the backing.  This helper centralises that decision so consumers stop
/// hand-rolling `instance_builtin_data` + per-type whitelists at 70+ sites.
///
/// Returns `Some(backing)` when `v` is a builtin-subclass instance whose
/// backing value (any builtin type — scalars, containers, AND
/// `BuiltinObject`-backed `bytearray`/`frozenset`) should act as the effective
/// receiver for the operation, i.e. the subclass uses the *inherited* builtin
/// behaviour for it.  Returns `None` for a plain value, a non-builtin
/// instance, or a subclass that **overrides** one of `override_dunders`.
///
/// The override gate is the correctness landmine: if the subclass defines its
/// own `__repr__`/`__iter__`/`__add__`/`__eq__`/… the *user* slot wins
/// (CPython: a subclass slot overrides the inherited one), so the consumer
/// must keep treating the instance as a `PyInstance` and dispatch the user
/// method.  Pass the dunder(s) relevant to the consumer; pass `&[]` only when
/// the operation is dunder-free (pure data extraction, e.g. `bytes(x)`'s
/// buffer read).
///
/// Builtin slots are exposed as attrs on the canonical primitive/`object`
/// classes.  Whether a resolved slot is inherited therefore comes from the
/// class that owns the first MRO definition, not from parsing the callable's
/// display/dispatch name.  This distinction matters when a subclass explicitly
/// assigns a builtin descriptor, for example `__eq__ = object.__eq__`: that is
/// an override even though the stored callable has the same value as the
/// descriptor inherited from `object`.
///
/// Hot-path note: the only work for a non-`PyInstance` operand is the
/// `as_py_instance_rc()` tag check, which returns `None` immediately.  Place
/// the call at the HEAD of a consumer's slow/fallback arm (after the
/// primitive fast paths) so plain `int`/`str`/`list` operands pay nothing.
#[inline]
pub(crate) fn effective_builtin_receiver(v: &Value, override_dunders: &[&str]) -> Option<Value> {
    let backing = builtin_data_backing(v)?;
    if override_dunders.is_empty() {
        return Some(backing);
    }
    let inst_rc = v.as_py_instance_rc()?;
    let class = Rc::clone(&inst_rc.borrow().class);
    for dunder in override_dunders {
        if !class_uses_inherited_builtin_slot(&class, dunder) {
            // Any definition owned by a non-canonical class is an override.
            // In particular, do not mistake a builtin descriptor explicitly
            // copied into a subclass's own dict for the inherited base slot.
            //
            // Canonical primitive classes are immutable, so their own attrs
            // cannot be replaced by Python code and are safe provenance for
            // the backing implementation.
            return None;
        }
    }
    Some(backing)
}

/// Whether the first MRO definition of `name` is owned by a canonical runtime
/// primitive (or `object`) rather than copied/defined on a subclass.
///
/// An absent slot is treated like inherited behaviour: callers use this only
/// after finding a builtin backing, and the backing operation remains the
/// canonical fallback when the subclass supplies no override.
fn class_uses_inherited_builtin_slot(class: &Rc<RefCell<PyClass>>, name: &str) -> bool {
    lookup_class_attr_owner(class, name).is_none_or(|owner| {
        primitive_class_kind(&owner).is_some() || Rc::ptr_eq(&owner, &object_class_singleton())
    })
}

/// True if `method_val` is the inherited `__iter__` sentinel installed on a
/// canonical primitive class by `build_primitive_classes` (issue #2324).
///
/// The value-kind check confirms this is the builtin sentinel representation;
/// the owner lookup distinguishes true inheritance from an explicit
/// `Subclass.__iter__ = bytes.__iter__` assignment of that same value.
pub(crate) fn is_inherited_builtin_iter_sentinel(
    class: &Rc<RefCell<PyClass>>,
    method_val: &Value,
) -> bool {
    matches!(method_val.kind(), ValueKind::BuiltinFunction(_))
        && class_uses_inherited_builtin_slot(class, "__iter__")
}
