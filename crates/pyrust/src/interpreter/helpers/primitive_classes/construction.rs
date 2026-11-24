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

/// Read the optional immutable identity carried by the six canonical iterator
/// classes without conflating an ordinary untagged class with a borrow
/// conflict. Call-cache code may still dispatch an exact primitive identity
/// during a conflict, but deliberately declines to retain metadata it could
/// not inspect.
#[inline]
pub(crate) fn try_builtin_type_class_tag(
    class: &Rc<RefCell<PyClass>>,
) -> std::result::Result<Option<pyrust_core::BuiltinTypeClassTag>, std::cell::BorrowError> {
    class.try_borrow().map(|class| class.builtin_type_tag)
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

fn bytearray_iterator_ctor(_interp: &mut Interpreter, _args: &[ExpandedCallArg]) -> Result<Value> {
    Err(PyError::named(
        "TypeError",
        "cannot create 'bytearray_iterator' instances".to_string(),
    ))
}

fn deque_iterator_ctor(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    crate::builtin_modules::collections::deque_iterator_constructor(interp, args, false)
}

fn deque_reverse_iterator_ctor(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    crate::builtin_modules::collections::deque_iterator_constructor(interp, args, true)
}

/// Parse `(name, doc=None)` arguments for `types.ModuleType(...)` /
/// `module.__init__(...)` with exact CPython 3.12 error messages (issue #2970).
fn parse_module_init_args(args: &[ExpandedCallArg]) -> Result<(String, Value)> {
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("module() takes at most 2 arguments ({} given)", args.len()),
        ));
    }
    let mut name_arg: Option<Value> = None;
    let mut doc_arg: Option<Value> = None;
    let mut unknown_kw: Option<String> = None;
    let mut pos_idx = 0usize;
    for arg in args {
        match arg.name.as_deref() {
            None => {
                match pos_idx {
                    0 => name_arg = Some(arg.value.clone()),
                    1 => doc_arg = Some(arg.value.clone()),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("module() takes at most 2 arguments ({} given)", args.len()),
                        ));
                    }
                }
                pos_idx += 1;
            }
            Some("name") => {
                if name_arg.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for module() given by name ('name') and position (1)".to_string(),
                    ));
                }
                name_arg = Some(arg.value.clone());
            }
            Some("doc") => {
                if doc_arg.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for module() given by name ('doc') and position (2)".to_string(),
                    ));
                }
                doc_arg = Some(arg.value.clone());
            }
            Some(other) => {
                if unknown_kw.is_none() {
                    unknown_kw = Some(other.to_string());
                }
            }
        }
    }
    // CPython validates required `name` before reporting an invalid keyword
    // argument (`ModuleType(foo=1)` -> missing required argument 'name').
    let Some(name_val) = name_arg else {
        return Err(PyError::named(
            "TypeError",
            "module() missing required argument 'name' (pos 1)".to_string(),
        ));
    };
    if let Some(bad_kw) = unknown_kw {
        return Err(PyError::named(
            "TypeError",
            format!("'{bad_kw}' is an invalid keyword argument for module()"),
        ));
    }
    let Some(name_str) = name_val.as_str().map(str::to_string) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "module() argument 'name' must be str, not {}",
                value_type_name_str(&name_val)
            ),
        ));
    };
    Ok((name_str, doc_arg.unwrap_or_else(Value::none)))
}

/// Constructor for `types.ModuleType(name, doc=None)` (issue #2970).
///
/// Allocates a real `PyModule` with CPython's five module-object dunder slots
/// in exact order (`__name__`, `__doc__`, `__package__`, `__loader__`,
/// `__spec__`) backed by a live Python dictionary.
fn module_ctor(_interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (name_str, doc_val) = parse_module_init_args(args)?;
    let mut attrs = crate::value::ModuleAttrs::default();
    attrs.insert("__name__".to_string(), Value::string(&name_str));
    attrs.insert("__doc__".to_string(), doc_val);
    attrs.insert("__package__".to_string(), Value::none());
    attrs.insert("__loader__".to_string(), Value::none());
    attrs.insert("__spec__".to_string(), Value::none());
    let mut module = crate::value::PyModule::new(name_str, attrs);
    module.attach_live_namespace();
    Ok(Value::py_module(Rc::new(RefCell::new(module))))
}

/// `module.__init__(self, name, doc=None)` for both direct `PyModule` objects
/// and Python subclasses of `types.ModuleType` (issue #2970).
pub(crate) fn call_module_init(args: &[ExpandedCallArg]) -> Result<Value> {
    let Some(first) = args.first().filter(|arg| arg.name.is_none()) else {
        return Err(PyError::named(
            "TypeError",
            "descriptor '__init__' of 'module' object needs an argument".to_string(),
        ));
    };
    let receiver = &first.value;
    let rest = &args[1..];
    match receiver.kind() {
        ValueKind::PyModule(module) => {
            let (name_str, doc_val) = parse_module_init_args(rest)?;
            let mut m = module.borrow_mut();
            m.name = name_str.clone();
            m.insert_attr("__name__".to_string(), Value::string(&name_str));
            m.insert_attr("__doc__".to_string(), doc_val);
            m.insert_attr("__package__".to_string(), Value::none());
            m.insert_attr("__loader__".to_string(), Value::none());
            m.insert_attr("__spec__".to_string(), Value::none());
            Ok(Value::none())
        }
        ValueKind::PyInstance(instance)
            if class_is_subclass_of(&instance.borrow().class, &module_type_singleton()) =>
        {
            let (name_str, doc_val) = parse_module_init_args(rest)?;
            let mut inst = instance.borrow_mut();
            inst.attrs.insert("__name__", Value::string(&name_str));
            inst.attrs.insert("__doc__", doc_val);
            inst.attrs.insert("__package__", Value::none());
            inst.attrs.insert("__loader__", Value::none());
            inst.attrs.insert("__spec__", Value::none());
            Ok(Value::none())
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "descriptor '__init__' requires a 'module' object but received a '{}'",
                value_type_name_str(receiver)
            ),
        )),
    }
}

/// `module.__repr__(self)` for both direct `PyModule` objects and Python
/// subclasses of `types.ModuleType` (issue #2970).
pub(crate) fn call_module_repr(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            "module.__repr__() takes no keyword arguments".to_string(),
        ));
    }
    let Some(first) = args.first() else {
        return Err(PyError::named(
            "TypeError",
            "descriptor '__repr__' of 'module' object needs an argument".to_string(),
        ));
    };
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("expected 0 arguments, got {}", args.len().saturating_sub(1)),
        ));
    }
    let receiver = &first.value;
    match receiver.kind() {
        ValueKind::PyModule(module) => Ok(Value::string(module.borrow().repr_string())),
        ValueKind::PyInstance(instance)
            if class_is_subclass_of(&instance.borrow().class, &module_type_singleton()) =>
        {
            let repr = match instance.borrow().attrs.get("__name__") {
                Some(v) if !v.is_unset() => format!("<module {}>", v.repr_raw()),
                _ => "<module '?'>".to_string(),
            };
            Ok(Value::string(repr))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "descriptor '__repr__' requires a 'module' object but received a '{}'",
                value_type_name_str(receiver)
            ),
        )),
    }
}

/// Whether `class` is an exact canonical collections.deque class.
///
/// The built-in module owns the reload-aware identity registry; runtime
/// protocol consumers use this typed interpreter facade instead of reaching
/// into a concrete module implementation.
pub(crate) fn is_canonical_deque_class(class: &Rc<RefCell<PyClass>>) -> bool {
    crate::builtin_modules::collections::is_canonical_deque_class(class)
}

pub(crate) fn is_deque_value(value: &Value) -> bool {
    crate::builtin_modules::collections::is_deque_value(value)
}

pub(crate) fn is_exact_deque_value(value: &Value) -> bool {
    crate::builtin_modules::collections::is_exact_deque_value(value)
}

pub(crate) fn deque_extend_in_place(
    interp: &mut Interpreter,
    target: &Value,
    iterable: &Value,
) -> Result<()> {
    crate::builtin_modules::collections::deque_extend_in_place(interp, target, iterable)
}

pub(crate) fn deque_concat_values(left: &Value, right: &Value) -> Result<Value> {
    crate::builtin_modules::collections::deque_concat_values(left, right)
}

pub(crate) fn deque_mul_by_i64(target: &Value, n: i64) -> Result<Value> {
    crate::builtin_modules::collections::deque_mul_by_i64(target, n)
}

pub(crate) fn deque_imul_by_i64(target: &Value, n: i64) -> Result<()> {
    crate::builtin_modules::collections::deque_imul_by_i64(target, n)
}

/// Preserve the module-owned, reload-aware identity policy for synthetic
/// `typing` values without exposing that implementation to runtime consumers.
pub(crate) fn mapping_proxy_typing_subscript_policy(value: &Value) -> Option<bool> {
    crate::builtin_modules::typing::mapping_proxy_subscript_policy(value)
}

/// Return a CPython-facing type name when a synthetic `typing` value's native
/// representation differs from the object CPython rejects.
pub(crate) fn mapping_proxy_typing_rejection_type_name(value: &Value) -> Option<&'static str> {
    crate::builtin_modules::typing::mapping_proxy_rejection_type_name(value)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BuiltinTypeInstanceProbe {
    Exact(BuiltinTypeClass),
    FixedOther,
    Dynamic,
}

/// Classify an object tested against issue #3000's six built-in classes.
///
/// `FixedOther` means the value's backing kind proves that none of the six can
/// match. `Dynamic` retains the normal typed-class path for subclass carriers,
/// provider iterators and class objects whose MRO or metatype is observable.
#[inline]
fn builtin_type_instance_probe(obj: &Value) -> BuiltinTypeInstanceProbe {
    match obj.kind() {
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => BuiltinTypeInstanceProbe::Dynamic,
        ValueKind::Generator(cell) => {
            // A running generator/coroutine frame owns a mutably-borrowed cell.
            // Its immutable kind tag proves it cannot be one of these built-in
            // iterators, so never inspect the state in that case (#2978).
            if cell.kind().frame_type_name().is_some() {
                return BuiltinTypeInstanceProbe::FixedOther;
            }
            // Built-in iterators release their state before executing user
            // code, making this borrow safe under the same invariant as
            // `value_class`.
            let state = cell.borrow();
            if state.downcast_ref::<ZipIter>().is_some() {
                return BuiltinTypeInstanceProbe::Exact(BuiltinTypeClass::Zip);
            }
            if state.downcast_ref::<MapIter>().is_some() {
                return BuiltinTypeInstanceProbe::Exact(BuiltinTypeClass::Map);
            }
            if state.downcast_ref::<FilterIter>().is_some() {
                return BuiltinTypeInstanceProbe::Exact(BuiltinTypeClass::Filter);
            }
            if state.downcast_ref::<EnumerateIter>().is_some() {
                return BuiltinTypeInstanceProbe::Exact(BuiltinTypeClass::Enumerate);
            }
            if state.downcast_ref::<ProviderIterator>().is_some() {
                return BuiltinTypeInstanceProbe::Dynamic;
            }
            if let Some(iterator) = state.downcast_ref::<GetItemIter>() {
                return if iterator.step < 0 {
                    BuiltinTypeInstanceProbe::Exact(BuiltinTypeClass::Reversed)
                } else {
                    BuiltinTypeInstanceProbe::FixedOther
                };
            }
            if let Some(iterator) = state.downcast_ref::<NativeIterFrame>() {
                return if iterator.class == Some(NativeIteratorClass::Reversed) {
                    BuiltinTypeInstanceProbe::Exact(BuiltinTypeClass::Reversed)
                } else {
                    BuiltinTypeInstanceProbe::FixedOther
                };
            }
            if state.downcast_ref::<CallableIter>().is_some()
                || state.downcast_ref::<RangeIter>().is_some()
                || state.downcast_ref::<BigRangeIter>().is_some()
                || state.downcast_ref::<AsyncGenASend>().is_some()
            {
                return BuiltinTypeInstanceProbe::FixedOther;
            }
            // Unknown type-erased states are conservative: a future provider
            // may retain a Python class even when this helper does not know its
            // concrete cursor type yet.
            BuiltinTypeInstanceProbe::Dynamic
        }
        ValueKind::BuiltinObject { ops, .. } if pyrust_builtins::slice::is_slice_ops(ops) => {
            BuiltinTypeInstanceProbe::Exact(BuiltinTypeClass::Slice)
        }
        _ => BuiltinTypeInstanceProbe::FixedOther,
    }
}

/// Preserve the existing low-level six-class probe contract for constructor
/// and iterator callers: only an exact backing kind produces `Some`.
#[inline]
fn builtin_type_class_for_isinstance(obj: &Value) -> Option<BuiltinTypeClass> {
    match builtin_type_instance_probe(obj) {
        BuiltinTypeInstanceProbe::Exact(kind) => Some(kind),
        BuiltinTypeInstanceProbe::FixedOther | BuiltinTypeInstanceProbe::Dynamic => None,
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

#[inline]
fn primitive_tag_isinstance(obj: &Value, tag: PrimitiveClassKind) -> bool {
    match tag {
        pyrust_core::CanonicalClassTag::Int => matches!(
            obj.kind(),
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
        ),
        pyrust_core::CanonicalClassTag::Bool => {
            matches!(obj.kind(), ValueKind::Bool(_))
        }
        pyrust_core::CanonicalClassTag::Str => matches!(obj.kind(), ValueKind::Str(_)),
        pyrust_core::CanonicalClassTag::Float => matches!(obj.kind(), ValueKind::Float(_)),
        pyrust_core::CanonicalClassTag::List => matches!(obj.kind(), ValueKind::List(_)),
        pyrust_core::CanonicalClassTag::Tuple => matches!(obj.kind(), ValueKind::Tuple(_)),
        pyrust_core::CanonicalClassTag::Dict => match obj.kind() {
            ValueKind::Dict(_) => true,
            ValueKind::BuiltinObject { ops, .. } => {
                ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Dict)
            }
            _ => false,
        },
        pyrust_core::CanonicalClassTag::Set => matches!(obj.kind(), ValueKind::Set(_)),
        pyrust_core::CanonicalClassTag::Bytes => matches!(obj.kind(), ValueKind::Bytes(_)),
        pyrust_core::CanonicalClassTag::Bytearray => matches!(
            obj.kind(),
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag()
                    == Some(pyrust_core::CanonicalClassTag::Bytearray)
        ),
        pyrust_core::CanonicalClassTag::Complex => {
            matches!(obj.kind(), ValueKind::Complex(_, _))
        }
        pyrust_core::CanonicalClassTag::Frozenset => matches!(
            obj.kind(),
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag()
                    == Some(pyrust_core::CanonicalClassTag::Frozenset)
        ),
        pyrust_core::CanonicalClassTag::MappingProxy => matches!(
            obj.kind(),
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag()
                    == Some(pyrust_core::CanonicalClassTag::MappingProxy)
        ),
        pyrust_core::CanonicalClassTag::NoneType => matches!(obj.kind(), ValueKind::None),
        pyrust_core::CanonicalClassTag::NotImplementedType => {
            matches!(obj.kind(), ValueKind::NotImplemented)
        }
        pyrust_core::CanonicalClassTag::Ellipsis => {
            matches!(obj.kind(), ValueKind::Ellipsis)
        }
        pyrust_core::CanonicalClassTag::Object
        | pyrust_core::CanonicalClassTag::GenericAlias
        | pyrust_core::CanonicalClassTag::TypeVar => false,
    }
}

/// Primitive-only fast path retained for internal callers that enter
/// `isinstance_single` without the public protocol preflight.
#[inline]
pub(crate) fn primitive_class_isinstance_fast(
    obj: &Value,
    cls: &Rc<RefCell<PyClass>>,
) -> Option<bool> {
    if matches!(obj.kind(), ValueKind::PyInstance(_) | ValueKind::PyClass(_)) {
        return None;
    }
    let tag = cls
        .borrow()
        .canonical_tag
        .filter(|tag| tag.is_primitive())?;
    Some(primitive_tag_isinstance(obj, tag))
}

/// Fast `isinstance` check for every interpreter-owned class with immutable
/// identity metadata.
///
/// Both class tags are read under one short borrow. Primitive classes answer
/// from `ValueKind`; the six iterator/slice classes use a tri-state backing
/// probe so fixed values settle while dynamic class carriers retain their MRO.
#[inline]
pub(crate) fn canonical_class_isinstance_fast(
    obj: &Value,
    cls: &Rc<RefCell<PyClass>>,
) -> Option<bool> {
    // A Python instance can subclass a primitive or one of the five
    // subclassable iterator classes. A class object is an instance of its
    // metatype. Both therefore require the typed-class path.
    if matches!(obj.kind(), ValueKind::PyInstance(_) | ValueKind::PyClass(_)) {
        return None;
    }

    let (canonical_tag, builtin_type_tag) = {
        let borrowed = cls.borrow();
        (borrowed.canonical_tag, borrowed.builtin_type_tag)
    };
    if let Some(tag) = canonical_tag.filter(|tag| tag.is_primitive()) {
        return Some(primitive_tag_isinstance(obj, tag));
    }
    let expected = BuiltinTypeClass::from_tag(builtin_type_tag?);
    match builtin_type_instance_probe(obj) {
        BuiltinTypeInstanceProbe::Exact(actual) => Some(actual == expected),
        BuiltinTypeInstanceProbe::FixedOther => Some(false),
        BuiltinTypeInstanceProbe::Dynamic => None,
    }
}

/// Fast `issubclass` check when `classinfo` is one of issue #3000's exact
/// six built-in classes. Exact tagged candidates compare tags directly;
/// ordinary Python subclasses retain the typed MRO walk.
#[inline]
pub(crate) fn builtin_type_class_issubclass_fast(
    candidate: &Value,
    classinfo: &Rc<RefCell<PyClass>>,
) -> Option<bool> {
    let expected = classinfo.borrow().builtin_type_tag?;
    let ValueKind::PyClass(candidate) = candidate.kind() else {
        return None;
    };
    let actual = candidate.borrow().builtin_type_tag;
    Some(match actual {
        Some(actual) => actual == expected,
        None => class_is_subclass_of(candidate, classinfo),
    })
}

/// Does this class inherit a native built-in layout whose allocator cannot be
/// bypassed through `object.__new__`?
///
/// The immutable tag identifies issue #3000's six built-in class singletons;
/// dictionary views and concrete native iterators use their typed singleton
/// identity because their final classes carry no public built-in layout tag.
/// Walking the class graph distinguishes genuine descendants from unrelated
/// same-named user classes. `slice` and dictionary views cannot have
/// descendants, but their exact classes must still reject bare allocation.
#[inline]
pub(crate) fn class_has_native_builtin_type_ancestor(class: &Rc<RefCell<PyClass>>) -> bool {
    let borrowed = class.borrow();
    if borrowed.builtin_type_tag.is_some()
        || DictViewClass::from_class(class).is_some()
        || NativeIteratorClass::from_class(class).is_some()
    {
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
