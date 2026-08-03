/// Fast-path dispatch lookup for primitive classes (#462 perf).
/// Returns the registry's `BuiltinDispatchFn` for the constructor of
/// the named primitive (`int`, `str`, …), or `None` for any other
/// class.  Called from `call_function_expanded`'s `PyClass` arm to
/// skip the `call_class_expanded` PyInstance-alloc + `__init__`-walk
/// + recursive `call_function_expanded` chain — three layers of
///   dispatch collapsed into one `HashMap` lookup and one fn-pointer
///   call.
#[inline]
pub(crate) fn primitive_class_dispatch(
    class: &Rc<RefCell<PyClass>>,
) -> Option<crate::builtin_registry::BuiltinDispatchFn> {
    let ptr = Rc::as_ptr(class);
    PRIMITIVE_CLASS_DISPATCH.with(|m| m.borrow().get(&ptr).copied())
}

/// Construct the backing value for a primitive subclass from its immutable
/// canonical identity.
///
/// Construction code deliberately does not translate the identity back into a
/// Python-visible built-in name. The helper layer owns the canonical singleton
/// and constructor-dispatch association.
#[inline]
pub(crate) fn construct_primitive_backing(
    interp: &mut Interpreter,
    kind: PrimitiveClassKind,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let class = canonical_class_by_tag(kind);
    let dispatch = primitive_class_dispatch(&class).ok_or_else(|| {
        PyError::Runtime(format!(
            "missing constructor dispatch for canonical primitive {kind:?}"
        ))
    })?;
    dispatch(interp, args)
}

/// Constructor for `NoneType` (issue #1451).
///
/// CPython 3.12: `type(None)()` returns `None`; any arguments raise
/// `TypeError: NoneType takes no arguments`.
fn none_ctor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "NoneType takes no arguments".to_string(),
        ));
    }
    Ok(Value::none())
}

/// Constructor for `NotImplementedType` (issue #1451).
///
/// CPython 3.12: `type(NotImplemented)()` returns `NotImplemented`; any
/// arguments raise `TypeError: NotImplementedType takes no arguments`.
fn notimplemented_ctor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "NotImplementedType takes no arguments".to_string(),
        ));
    }
    Ok(Value::not_implemented())
}

/// Constructor for `ellipsis` (issue #1451).
///
/// CPython 3.12: `type(...)()` returns `Ellipsis`; any arguments raise
/// `TypeError: EllipsisType takes no arguments`.
fn ellipsis_ctor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "EllipsisType takes no arguments".to_string(),
        ));
    }
    Ok(Value::ellipsis())
}

fn dict_view_ctor_error(name: &str) -> Result<Value> {
    Err(PyError::named(
        "TypeError",
        format!("cannot create '{name}' instances"),
    ))
}

fn dict_keys_ctor(_interp: &mut Interpreter, _args: &[ExpandedCallArg]) -> Result<Value> {
    dict_view_ctor_error(pyrust_builtins::dict_views::DICT_KEYS_TYPE_NAME)
}

fn dict_items_ctor(_interp: &mut Interpreter, _args: &[ExpandedCallArg]) -> Result<Value> {
    dict_view_ctor_error(pyrust_builtins::dict_views::DICT_ITEMS_TYPE_NAME)
}

fn dict_values_ctor(_interp: &mut Interpreter, _args: &[ExpandedCallArg]) -> Result<Value> {
    dict_view_ctor_error(pyrust_builtins::dict_views::DICT_VALUES_TYPE_NAME)
}

fn odict_keys_ctor(_interp: &mut Interpreter, _args: &[ExpandedCallArg]) -> Result<Value> {
    dict_view_ctor_error(pyrust_builtins::dict_views::ODICT_KEYS_TYPE_NAME)
}

fn odict_items_ctor(_interp: &mut Interpreter, _args: &[ExpandedCallArg]) -> Result<Value> {
    dict_view_ctor_error(pyrust_builtins::dict_views::ODICT_ITEMS_TYPE_NAME)
}

fn odict_values_ctor(_interp: &mut Interpreter, _args: &[ExpandedCallArg]) -> Result<Value> {
    dict_view_ctor_error(pyrust_builtins::dict_views::ODICT_VALUES_TYPE_NAME)
}

/// Identify one of the iterator/slice values migrated to a real built-in class
/// in issue #3000 without materialising the class as a `Value`.
///
/// The caller only invokes the pointer table when this object-first probe
/// succeeds.  Ordinary primitive and user values therefore do not acquire six
/// more class-pointer comparisons on their `isinstance` miss path.
#[inline]
fn builtin_type_class_for_isinstance(obj: &Value) -> Option<BuiltinTypeClass> {
    match obj.kind() {
        ValueKind::Generator(cell) => {
            // A running generator/coroutine frame owns a mutably-borrowed cell.
            // Its immutable kind tag proves it cannot be one of these built-in
            // iterators, so never inspect the state in that case (#2978).
            if cell.kind().frame_type_name().is_some() {
                return None;
            }
            // Built-in iterators release their state before executing user
            // code, making this borrow safe under the same invariant as
            // `value_class`.
            let state = cell.borrow();
            if state.downcast_ref::<ZipIter>().is_some() {
                return Some(BuiltinTypeClass::Zip);
            }
            if state.downcast_ref::<MapIter>().is_some() {
                return Some(BuiltinTypeClass::Map);
            }
            if state.downcast_ref::<FilterIter>().is_some() {
                return Some(BuiltinTypeClass::Filter);
            }
            if state.downcast_ref::<EnumerateIter>().is_some() {
                return Some(BuiltinTypeClass::Enumerate);
            }
            if let Some(iterator) = state.downcast_ref::<GetItemIter>()
                && iterator.step < 0
            {
                return Some(BuiltinTypeClass::Reversed);
            }
            if let Some(iterator) = state.downcast_ref::<NativeIterFrame>()
                && iterator.type_name == "reversed"
            {
                return Some(BuiltinTypeClass::Reversed);
            }
            None
        }
        ValueKind::BuiltinObject { ops, .. } if pyrust_builtins::slice::is_slice_ops(ops) => {
            Some(BuiltinTypeClass::Slice)
        }
        _ => None,
    }
}

/// Fast `isinstance` check for issue #3000's six built-in type singletons.
///
/// Callers deliberately invoke this only from the existing `Generator` /
/// `BuiltinObject` arms of `isinstance_single`.  Keeping it outside the
/// primitive fast path means ordinary primitive and user values do not acquire
/// backing-kind probes.  The expected class's immutable tag is checked first,
/// so a native iterator tested against an unrelated user class also falls
/// through without inspecting its backing.
#[inline]
pub(crate) fn builtin_type_class_isinstance_fast(
    obj: &Value,
    cls: &Rc<RefCell<PyClass>>,
) -> Option<bool> {
    let expected_kind = BuiltinTypeClass::from_tag(cls.borrow().builtin_type_tag?);
    let actual_kind = builtin_type_class_for_isinstance(obj)?;
    Some(actual_kind == expected_kind)
}

/// Does this class inherit a native built-in layout whose allocator cannot be
/// bypassed through `object.__new__`?
///
/// The immutable tag identifies issue #3000's six built-in class singletons;
/// dictionary views use their typed singleton identity because their hidden,
/// final classes deliberately have no public constructor tag. Walking the
/// class graph distinguishes genuine descendants from unrelated same-named
/// user classes. `slice` and dictionary views cannot have descendants, but
/// their exact classes must still reject bare allocation.
#[inline]
pub(crate) fn class_has_native_builtin_type_ancestor(class: &Rc<RefCell<PyClass>>) -> bool {
    let borrowed = class.borrow();
    if borrowed.builtin_type_tag.is_some() || DictViewClass::from_class(class).is_some() {
        return true;
    }
    if let Some(base) = &borrowed.base
        && class_has_native_builtin_type_ancestor(base)
    {
        return true;
    }
    borrowed
        .extra_bases
        .iter()
        .any(class_has_native_builtin_type_ancestor)
}

/// Fast `isinstance(obj, primitive_class)` — when `cls` is one of the
/// canonical primitive class singletons, skip the `class_is_subclass_of`
/// walk (which would require materialising `obj`'s class via
/// `primitive_class_for_value`'s thread_local + Rc::clone) and do a
/// direct `ValueKind` tag check.  `Some(true/false)` on a hit,
/// `None` if `cls` isn't a primitive class — fall through to the
/// general walk.  Issue #462 perf.
#[inline]
pub(crate) fn primitive_class_isinstance_fast(
    obj: &Value,
    cls: &Rc<RefCell<PyClass>>,
) -> Option<bool> {
    // Issue #976: a PyInstance may subclass a primitive.  Skip the fast-path
    // tag check and return None so the caller falls through to the general
    // `class_is_subclass_of` MRO walk, which correctly handles
    // `isinstance(MyDict(), dict)` when MyDict inherits from dict.
    if matches!(obj.kind(), ValueKind::PyInstance(_)) {
        return None;
    }
    let cls_ptr = Rc::as_ptr(cls);
    PRIMITIVE_CLASSES.with(|c| {
        // bool ⊂ int: an int-class test matches both Int and Bool.
        // Every other primitive is a tag identity.
        if cls_ptr == Rc::as_ptr(&c.int_class) {
            return Some(matches!(
                obj.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            ));
        }
        if cls_ptr == Rc::as_ptr(&c.bool_class) {
            return Some(matches!(obj.kind(), ValueKind::Bool(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.str_class) {
            return Some(matches!(obj.kind(), ValueKind::Str(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.float_class) {
            return Some(matches!(obj.kind(), ValueKind::Float(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.list_class) {
            return Some(matches!(obj.kind(), ValueKind::List(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.tuple_class) {
            return Some(matches!(obj.kind(), ValueKind::Tuple(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.dict_class) {
            return Some(matches!(obj.kind(), ValueKind::Dict(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.set_class) {
            return Some(matches!(obj.kind(), ValueKind::Set(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.bytes_class) {
            return Some(matches!(obj.kind(), ValueKind::Bytes(_)));
        }
        if cls_ptr == Rc::as_ptr(&c.bytearray_class) {
            return Some(matches!(
                obj.kind(),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Bytearray)
            ));
        }
        if cls_ptr == Rc::as_ptr(&c.complex_class) {
            return Some(matches!(obj.kind(), ValueKind::Complex(_, _)));
        }
        if cls_ptr == Rc::as_ptr(&c.frozenset_class) {
            return Some(matches!(
                obj.kind(),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Frozenset)
            ));
        }
        if cls_ptr == Rc::as_ptr(&c.mappingproxy_class) {
            return Some(matches!(
                obj.kind(),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::MappingProxy)
            ));
        }
        if cls_ptr == Rc::as_ptr(&c.none_class) {
            return Some(matches!(obj.kind(), ValueKind::None));
        }
        if cls_ptr == Rc::as_ptr(&c.notimplemented_class) {
            return Some(matches!(obj.kind(), ValueKind::NotImplemented));
        }
        if cls_ptr == Rc::as_ptr(&c.ellipsis_class) {
            return Some(matches!(obj.kind(), ValueKind::Ellipsis));
        }
        None
    })
}
