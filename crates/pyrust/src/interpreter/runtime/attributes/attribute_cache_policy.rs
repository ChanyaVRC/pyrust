// Attribute cache semantic policy.
// Semantic policy queried by attribute inline caches. The fast-path domain
// owns cache state and hit execution; this module owns whether a Python
// attribute resolution may be represented by that cache.

enum DescriptorCacheKind {
    Plain,
    Dynamic,
    Slot(pyrust_core::MemberSlotId),
}

fn descriptor_cache_kind(value: &Value) -> DescriptorCacheKind {
    if let Some(info) = pyrust_builtins::member_descriptor::as_member_descriptor_full(value) {
        DescriptorCacheKind::Slot(info.slot_id)
    } else if pyrust_builtins::cached_property::with_cached_property(value, |_| ()).is_some()
        || pyrust_builtins::property::with_property(value, |_| ()).is_some()
        || pyrust_builtins::classmethod::as_class_method_any(value).is_some()
        || pyrust_builtins::classmethod::as_static_method_any(value).is_some()
    {
        DescriptorCacheKind::Dynamic
    } else {
        DescriptorCacheKind::Plain
    }
}

/// Resolution shape that a `GetAttr` inline cache may safely retain.
pub(crate) enum ReadAttributeCachePlan {
    Uncacheable,
    Instance,
    Slot(pyrust_core::MemberSlotId),
    Class(Value),
    NativeClassMethod(pyrust_builtins::classmethod::NativeClassMethodCachePlan),
}

/// Resolution shape that a `CallMethod` inline cache may safely retain.
pub(crate) enum ReadMethodCachePlan {
    Uncacheable,
    Class(Value),
    NativeClassMethod(pyrust_builtins::classmethod::NativeClassMethodCachePlan),
}

/// Classify the result of a completed generic attribute lookup for cache fill.
///
/// This keeps every semantic exclusion in the attribute domain: dynamic
/// descriptors, custom `__getattribute__`, lazy TypeVar fields, deferred
/// tracebacks, and numeric-tower descriptors must always take the regular
/// lookup path. The cache implementation only translates this typed plan into
/// its bytecode-local state.
pub(crate) fn read_attribute_cache_plan(target: &Value, name: &str) -> ReadAttributeCachePlan {
    if matches!(name, "__class__" | "__dict__") {
        return ReadAttributeCachePlan::Uncacheable;
    }

    if let ValueKind::PyClass(class) = target.kind() {
        // A data descriptor on the metaclass wins over the class MRO.  Native
        // classmethod plans are admitted only after the same precedence check
        // as `get_attr_class`, so metaclass monkeypatches cannot be bypassed.
        if metaclass_dunder(class, name).is_some_and(|value| is_data_descriptor(&value)) {
            return ReadAttributeCachePlan::Uncacheable;
        }
        let Some(descriptor) = lookup_class_attr(class, name) else {
            return ReadAttributeCachePlan::Uncacheable;
        };
        return pyrust_builtins::classmethod::native_class_method_cache_plan(&descriptor, class)
            .map(ReadAttributeCachePlan::NativeClassMethod)
            .unwrap_or(ReadAttributeCachePlan::Uncacheable);
    }

    let Some(instance) = target.as_py_instance_rc() else {
        return ReadAttributeCachePlan::Uncacheable;
    };
    let instance = instance.borrow();

    if matches!(name, "__bound__" | "__constraints__") && is_typevar_class(&instance.class) {
        return ReadAttributeCachePlan::Uncacheable;
    }
    if name == "__value__" && is_type_alias_class(&instance.class) {
        return ReadAttributeCachePlan::Uncacheable;
    }

    let has_custom_getattribute = lookup_class_attr(&instance.class, "__getattribute__")
        .is_some_and(|value| matches!(value.kind(), ValueKind::UserFunction(_)));
    if has_custom_getattribute {
        return ReadAttributeCachePlan::Uncacheable;
    }

    // Native exception slots have data-descriptor precedence over a
    // same-named visible dict key, but do not have materialised class
    // descriptors in pyrust. Never cache that dict key as an Instance hit.
    if active_exception_slot_policy(&instance.class, name).is_some() {
        return ReadAttributeCachePlan::Uncacheable;
    }

    // Member cells are not reported by `contains_key()` (which intentionally
    // describes only `__dict__`). Resolve the descriptor first because a
    // visible name is not a physical slot identity.
    if let Some(class_attr) = lookup_class_attr(&instance.class, name)
        && let DescriptorCacheKind::Slot(slot_id) = descriptor_cache_kind(&class_attr)
        && instance.attrs.get_member_slot(slot_id).is_some()
    {
        return ReadAttributeCachePlan::Slot(slot_id);
    }

    if instance.attrs.contains_key(name) {
        if matches!(
            name,
            "real" | "imag" | "numerator" | "denominator" | "__traceback__"
        ) {
            return ReadAttributeCachePlan::Uncacheable;
        }
        let Some(class_attr) = lookup_class_attr(&instance.class, name) else {
            return ReadAttributeCachePlan::Instance;
        };
        return match descriptor_cache_kind(&class_attr) {
            DescriptorCacheKind::Slot(slot_id) => ReadAttributeCachePlan::Slot(slot_id),
            DescriptorCacheKind::Dynamic => ReadAttributeCachePlan::Uncacheable,
            DescriptorCacheKind::Plain if is_data_descriptor(&class_attr) => {
                ReadAttributeCachePlan::Uncacheable
            }
            DescriptorCacheKind::Plain => ReadAttributeCachePlan::Instance,
        };
    }

    let Some(class_attr) = lookup_class_attr(&instance.class, name) else {
        return ReadAttributeCachePlan::Uncacheable;
    };
    match descriptor_cache_kind(&class_attr) {
        DescriptorCacheKind::Plain
            if !is_data_descriptor(&class_attr) && !is_non_data_descriptor(&class_attr) =>
        {
            ReadAttributeCachePlan::Class(class_attr)
        }
        DescriptorCacheKind::Plain
            if matches!(
                class_attr.kind(),
                ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
            ) =>
        {
            ReadAttributeCachePlan::Class(class_attr)
        }
        _ => ReadAttributeCachePlan::Uncacheable,
    }
}

/// Classify a completed method lookup for cache fill.
///
/// Instance method calls may retain only a regular Python function or builtin
/// function reached through the class, with no instance shadow and no custom
/// `__getattribute__`.  On a class target, only the explicit native
/// classmethod plan above is cacheable; ordinary Python descriptor binding
/// continues through generic lookup.
pub(crate) fn read_method_cache_plan(target: &Value, name: &str) -> ReadMethodCachePlan {
    match read_attribute_cache_plan(target, name) {
        ReadAttributeCachePlan::NativeClassMethod(plan) => {
            ReadMethodCachePlan::NativeClassMethod(plan)
        }
        ReadAttributeCachePlan::Class(value)
            if matches!(
                value.kind(),
                ValueKind::UserFunction(function)
                    if matches!(function.kind, UserFunctionKind::Regular)
            ) || matches!(value.kind(), ValueKind::BuiltinFunction(_)) =>
        {
            ReadMethodCachePlan::Class(value)
        }
        _ => ReadMethodCachePlan::Uncacheable,
    }
}

/// Return the target class when a completed `SetAttr` may populate the plain
/// instance-write cache.
pub(crate) fn write_attribute_cache_class(
    target: &Value,
    name: &str,
) -> Option<Rc<RefCell<PyClass>>> {
    if matches!(name, "__class__" | "__dict__") {
        return None;
    }
    let instance = target.as_py_instance_rc()?;
    let class = Rc::clone(&instance.borrow().class);
    let uses_default_setattr = lookup_class_attr(&class, "__setattr__").is_none_or(|value| {
        crate::interpreter::value_is_canonical_slot(
            &value,
            crate::interpreter::CanonicalSlot::ObjectSetAttr,
        )
    });
    let has_data_descriptor =
        lookup_class_attr(&class, name).is_some_and(|value| is_data_descriptor(&value));
    let has_slots = class.borrow().slots.is_some();
    let is_object_singleton = Rc::ptr_eq(&class, &object_class_singleton());
    let is_exception = is_exception_class(&class);

    (uses_default_setattr
        && !has_data_descriptor
        && !has_slots
        && !is_object_singleton
        && !is_exception)
        .then_some(class)
}
