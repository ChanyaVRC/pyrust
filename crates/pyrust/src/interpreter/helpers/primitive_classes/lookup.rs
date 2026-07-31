/// Returns the singleton `method` class.  In CPython, `type(instance.method)`
/// returns `<class 'method'>` and `type(type(c.m)) is type` holds because
/// `method` is a proper `PyClass` (not a `BuiltinFunction` sentinel).
/// Issue #1528.
pub(crate) fn method_type_singleton() -> Rc<RefCell<PyClass>> {
    METHOD_TYPE.with(Rc::clone)
}

/// Returns the singleton `function` class.  In CPython, `type(lambda: None)`
/// returns `<class 'function'>` and `type(type(lambda: None)) is type` holds.
/// Issue #1528.
pub(crate) fn function_type_singleton() -> Rc<RefCell<PyClass>> {
    FUNCTION_TYPE.with(Rc::clone)
}

/// Returns the singleton `range` class.  In CPython, `range` is a proper
/// type (`type(range(5)) is range`), not a builtin function.  This singleton
/// is registered in `PRIMITIVE_CLASS_DISPATCH` so that calling
/// `range(start, stop)` still dispatches to the existing registry fn, and
/// is linked into ABC `extra_bases` so `issubclass(range, Sequence)` works.
/// Issues #1793, #1800.
pub(crate) fn range_class_singleton() -> Rc<RefCell<PyClass>> {
    RANGE_CLASS.with(Rc::clone)
}

/// Returns the singleton `types.GenericAlias` class.  In CPython 3.12,
/// `type(list[int])` is `<class 'types.GenericAlias'>` — a proper class with
/// `__name__ == "GenericAlias"` and `__module__ == "types"`, so
/// `type(type(list[int])) is type` holds.  Issue #2733.
pub(crate) fn generic_alias_class_singleton() -> Rc<RefCell<PyClass>> {
    GENERIC_ALIAS_CLASS.with(Rc::clone)
}

/// Returns this thread's singleton for one of the `builtins` types that own no
/// primitive storage variant (`zip`, `map`, `filter`, `enumerate`, `slice`,
/// `reversed`), or `None` for any other name.  Issue #3000.
pub(crate) fn builtin_type_class_by_name(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    BuiltinTypeClass::ALL
        .into_iter()
        .find(|kind| kind.class_name() == name)
        .map(BuiltinTypeClass::singleton)
}

/// True when `class` is an interpreter-owned `builtins` type singleton that
/// carries no [`CanonicalClassTag`](pyrust_core::CanonicalClassTag) — `range`
/// plus the [`BuiltinTypeClass`] group.  Such a class still has the built-in
/// type metadata (`__module__ == "builtins"`, a `__doc__` from
/// [`builtin_class_doc`]) that `type` supplies virtually.
pub(crate) fn is_untagged_builtin_type_class(class: &Rc<RefCell<PyClass>>) -> bool {
    let ptr = Rc::as_ptr(class);
    RANGE_CLASS.with(|range| ptr == Rc::as_ptr(range))
        || BUILTIN_TYPE_CLASSES
            .with(|classes| classes.iter().any(|entry| ptr == Rc::as_ptr(entry)))
}

/// Look up the per-primitive `PyClass` singleton for one of the migrated
/// primitive type names (`int`, `str`, `list`, …).  Returns `None` for any
/// other name — callers fall through to the legacy `BuiltinFunction(name)`
/// path.  See [`PRIMITIVE_CLASSES`].
pub(crate) fn primitive_class_by_name(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    if name == "range" {
        return Some(RANGE_CLASS.with(Rc::clone));
    }
    if let Some(class) = builtin_type_class_by_name(name) {
        return Some(class);
    }
    PRIMITIVE_CLASSES.with(|c| {
        Some(Rc::clone(match name {
            "bool" => &c.bool_class,
            "bytearray" => &c.bytearray_class,
            "bytes" => &c.bytes_class,
            "complex" => &c.complex_class,
            "dict" => &c.dict_class,
            "ellipsis" => &c.ellipsis_class,
            "float" => &c.float_class,
            "frozenset" => &c.frozenset_class,
            "int" => &c.int_class,
            "list" => &c.list_class,
            "mappingproxy" => &c.mappingproxy_class,
            "NoneType" => &c.none_class,
            "NotImplementedType" => &c.notimplemented_class,
            "set" => &c.set_class,
            "str" => &c.str_class,
            "tuple" => &c.tuple_class,
            _ => return None,
        }))
    })
}

/// Resolve an immutable canonical class tag back to its interpreter-owned
/// class singleton.
///
/// Protocol code should prefer this typed direction over parsing
/// Python-visible class or builtin-function names. This also includes
/// `object`, which [`primitive_class_kind`] intentionally excludes.
pub(crate) fn canonical_class_by_tag(tag: pyrust_core::CanonicalClassTag) -> Rc<RefCell<PyClass>> {
    if tag == pyrust_core::CanonicalClassTag::Object {
        return object_class_singleton();
    }
    PRIMITIVE_CLASSES.with(|classes| {
        Rc::clone(match tag {
            pyrust_core::CanonicalClassTag::Object => unreachable!("handled above"),
            pyrust_core::CanonicalClassTag::Bool => &classes.bool_class,
            pyrust_core::CanonicalClassTag::Bytearray => &classes.bytearray_class,
            pyrust_core::CanonicalClassTag::Bytes => &classes.bytes_class,
            pyrust_core::CanonicalClassTag::Complex => &classes.complex_class,
            pyrust_core::CanonicalClassTag::Dict => &classes.dict_class,
            pyrust_core::CanonicalClassTag::Ellipsis => &classes.ellipsis_class,
            pyrust_core::CanonicalClassTag::Float => &classes.float_class,
            pyrust_core::CanonicalClassTag::Frozenset => &classes.frozenset_class,
            pyrust_core::CanonicalClassTag::GenericAlias => {
                return generic_alias_class_singleton();
            }
            pyrust_core::CanonicalClassTag::Int => &classes.int_class,
            pyrust_core::CanonicalClassTag::List => &classes.list_class,
            pyrust_core::CanonicalClassTag::MappingProxy => &classes.mappingproxy_class,
            pyrust_core::CanonicalClassTag::NoneType => &classes.none_class,
            pyrust_core::CanonicalClassTag::NotImplementedType => &classes.notimplemented_class,
            pyrust_core::CanonicalClassTag::Set => &classes.set_class,
            pyrust_core::CanonicalClassTag::Str => &classes.str_class,
            pyrust_core::CanonicalClassTag::Tuple => &classes.tuple_class,
        })
    })
}

/// Return the `PyClass` that `type(v)` should yield for primitive types.
/// Returns `None` for variants that aren't part of this migration (functions,
/// modules, instances, …) — the caller falls back to its existing per-variant
/// logic.
pub(crate) fn primitive_class_for_value(v: &Value) -> Option<Rc<RefCell<PyClass>>> {
    let tag = match v.kind() {
        ValueKind::Bool(_) => pyrust_core::CanonicalClassTag::Bool,
        ValueKind::Int(_) | ValueKind::BigInt(_) => pyrust_core::CanonicalClassTag::Int,
        ValueKind::Float(_) => pyrust_core::CanonicalClassTag::Float,
        ValueKind::Str(_) => pyrust_core::CanonicalClassTag::Str,
        ValueKind::List(_) => pyrust_core::CanonicalClassTag::List,
        ValueKind::Tuple(_) => pyrust_core::CanonicalClassTag::Tuple,
        ValueKind::Dict(_) => pyrust_core::CanonicalClassTag::Dict,
        ValueKind::Set(_) => pyrust_core::CanonicalClassTag::Set,
        ValueKind::Bytes(_) => pyrust_core::CanonicalClassTag::Bytes,
        ValueKind::Complex(_, _) => pyrust_core::CanonicalClassTag::Complex,
        ValueKind::None => pyrust_core::CanonicalClassTag::NoneType,
        ValueKind::NotImplemented => pyrust_core::CanonicalClassTag::NotImplementedType,
        ValueKind::Ellipsis => pyrust_core::CanonicalClassTag::Ellipsis,
        ValueKind::Range { .. } | ValueKind::BigRange { .. } => {
            return Some(RANGE_CLASS.with(Rc::clone));
        }
        ValueKind::BuiltinObject { ops, .. } => ops.canonical_class_tag()?,
        _ => return None,
    };
    Some(canonical_class_by_tag(tag))
}

/// True iff `class` is one of the 11 migrated-primitive class singletons.
/// O(1) via the [`PRIMITIVE_CLASS_DISPATCH`] table.
pub(crate) fn is_primitive_class(class: &Rc<RefCell<PyClass>>) -> bool {
    primitive_class_dispatch(class).is_some()
}
