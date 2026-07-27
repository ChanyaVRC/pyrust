// Shared attribute-domain support.
/// Resolve the built-in exception family's native slot policy for this class
/// lookup.
///
/// A user exception subclass may override a native slot name with its own
/// class attribute. In that case normal descriptor/instance-dict precedence
/// applies and the internal slot remains available only to exception
/// machinery (for example `BaseException.__reduce__`).
fn active_exception_slot_policy(
    class: &Rc<RefCell<PyClass>>,
    name: &str,
) -> Option<ExceptionSlotPolicy> {
    let policy = exception_slot_policy(class, name)?;
    lookup_class_attr(class, name).is_none().then_some(policy)
}

#[derive(PartialEq, Eq)]
struct InstanceLayoutAdditions {
    names: Vec<String>,
}

fn class_layout_features(class: &Rc<RefCell<PyClass>>) -> (bool, bool) {
    let (tag, builtin_exception, slots, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.canonical_tag,
            borrowed.builtin_exception_name,
            borrowed.slots.clone(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if tag == Some(pyrust_core::CanonicalClassTag::Object) {
        return (false, false);
    }
    // BaseException has a native instance dict but no weakref slot. Every
    // built-in exception subclass inherits that layout.
    if builtin_exception.is_some() {
        return (true, false);
    }
    // Other canonical/native builtins are layout anchors. Their user
    // subclasses may add storage, but the native object itself contributes no
    // Python heap-type dict/weakref fields in pyrust's class model.
    if tag.is_some() {
        return (false, false);
    }
    let inherited = base
        .iter()
        .chain(extra_bases.iter())
        .map(class_layout_features)
        .fold((false, false), |(dict, weak), (base_dict, base_weak)| {
            (dict || base_dict, weak || base_weak)
        });
    match slots {
        None => (true, true),
        Some(names) => (
            inherited.0 || names.iter().any(|name| name == "__dict__"),
            inherited.1 || names.iter().any(|name| name == "__weakref__"),
        ),
    }
}

fn instance_layout_additions(class: &Rc<RefCell<PyClass>>) -> InstanceLayoutAdditions {
    let (slots, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.slots.clone(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    let inherited = base
        .iter()
        .chain(extra_bases.iter())
        .map(class_layout_features)
        .fold((false, false), |(dict, weak), (base_dict, base_weak)| {
            (dict || base_dict, weak || base_weak)
        });
    let mut names = match slots {
        Some(names) => names.into_iter().collect(),
        None => {
            let mut names = Vec::with_capacity(2);
            if !inherited.0 {
                names.push("__dict__".to_string());
            }
            if !inherited.1 {
                names.push("__weakref__".to_string());
            }
            names
        }
    };
    names.sort_unstable();
    names.dedup();
    InstanceLayoutAdditions { names }
}

fn is_native_layout_anchor(class: &Rc<RefCell<PyClass>>) -> bool {
    let borrowed = class.borrow();
    borrowed.canonical_tag.is_some() || borrowed.builtin_exception_name.is_some()
}

/// Find the first class in the primary-base chain that actually adds instance
/// storage. Layout-neutral heap classes (`__slots__ = ()`, or an ordinary
/// subclass whose base already supplies dict/weakref storage) are skipped,
/// matching CPython's `compatible_with_tp_base` walk.
fn effective_layout_root(mut class: Rc<RefCell<PyClass>>) -> Rc<RefCell<PyClass>> {
    loop {
        if is_native_layout_anchor(&class) || !instance_layout_additions(&class).names.is_empty() {
            return class;
        }
        let Some(base) = class.borrow().base.clone() else {
            return class;
        };
        class = base;
    }
}

fn same_optional_class(
    left: Option<&Rc<RefCell<PyClass>>>,
    right: Option<&Rc<RefCell<PyClass>>>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Rc::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

/// CPython-compatible heap-type layout check for `obj.__class__ = NewType`.
///
/// Two distinct classes may exchange instances when their effective
/// layout-adding layer has the same direct bases and adds the same set of
/// slots. A native layout anchor (exception/primitive/object) is compatible
/// only with itself.
fn instance_layouts_compatible(old: &Rc<RefCell<PyClass>>, new: &Rc<RefCell<PyClass>>) -> bool {
    if Rc::ptr_eq(old, new) {
        return true;
    }
    let old_root = effective_layout_root(Rc::clone(old));
    let new_root = effective_layout_root(Rc::clone(new));
    if Rc::ptr_eq(&old_root, &new_root) {
        return true;
    }
    if is_native_layout_anchor(&old_root) || is_native_layout_anchor(&new_root) {
        return false;
    }
    let (old_base, old_extra) = {
        let borrowed = old_root.borrow();
        (borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    let (new_base, new_extra) = {
        let borrowed = new_root.borrow();
        (borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    same_optional_class(old_base.as_ref(), new_base.as_ref())
        && old_extra.len() == new_extra.len()
        && old_extra
            .iter()
            .zip(&new_extra)
            .all(|(left, right)| Rc::ptr_eq(left, right))
        && instance_layout_additions(&old_root) == instance_layout_additions(&new_root)
}

/// Map physical member cells between the two distinct local layout layers
/// accepted by [`instance_layouts_compatible`].
///
/// A member descriptor identity belongs to its declaring class. Compatible
/// sibling layouts may declare the same slot names in a different order, so
/// `__class__` reassignment preserves values by local name while inherited
/// descriptor identities remain untouched.
fn member_slot_retype_remap(
    old: &Rc<RefCell<PyClass>>,
    new: &Rc<RefCell<PyClass>>,
) -> Vec<(pyrust_core::MemberSlotId, pyrust_core::MemberSlotId)> {
    let old_root = effective_layout_root(Rc::clone(old));
    let new_root = effective_layout_root(Rc::clone(new));
    if Rc::ptr_eq(&old_root, &new_root) {
        return Vec::new();
    }

    let old_members = old_root
        .borrow()
        .attrs
        .iter()
        .filter_map(|(name, descriptor)| {
            pyrust_builtins::member_descriptor::as_member_descriptor_full(descriptor)
                .map(|info| (name.clone(), info.slot_id))
        })
        .collect::<Vec<_>>();
    let new_borrowed = new_root.borrow();
    old_members
        .into_iter()
        .filter_map(|(name, old_id)| {
            let descriptor = new_borrowed.attrs.get(&name)?;
            let new_id =
                pyrust_builtins::member_descriptor::as_member_descriptor_full(descriptor)?.slot_id;
            Some((old_id, new_id))
        })
        .collect()
}

/// CPython's `tp_name`-style display name for a class used in descriptor error
/// messages: `<module>.<qualname>`, dropping the module prefix when it is
/// `builtins` or absent (issue #2479).  `OrderedDict` → `collections.OrderedDict`.
pub(crate) fn class_descriptor_display_name(class: &Rc<RefCell<PyClass>>) -> String {
    let borrowed = class.borrow();
    let qualname = borrowed.qualname.clone();
    let module = borrowed
        .attrs
        .get("__module__")
        .and_then(|m| match m.kind() {
            ValueKind::Str(s) => Some(s.to_string()),
            _ => None,
        });
    match module {
        Some(m) if m != "builtins" && !m.is_empty() => format!("{m}.{qualname}"),
        _ => qualname,
    }
}

/// Returns the attrs `Rc` for `func`, initialising it lazily on first call.
///
/// Lazy init avoids two heap allocations per function definition for the common
/// case where no attrs are ever set.  Interior mutability (`RefCell`) allows
/// initialization through a shared `Rc<UserFunction>`.
fn func_attrs_rc(func: &UserFunction) -> Rc<RefCell<Value>> {
    let mut slot = func.attrs.borrow_mut();
    if slot.is_none() {
        *slot = Some(Rc::new(RefCell::new(Value::dict(PyDict::default()))));
    }
    Rc::clone(slot.as_ref().unwrap())
}

/// Handle attribute lookup on a bound method for attributes that are shared
/// between `BoundMethod` and `ClassBoundMethod` (everything except `__func__`
/// and `__self__` which differ between the two variants).
///
/// Returns `Some(Ok(v))` when the attribute was found, `Some(Err(_))` if it
/// raised, or `None` to signal fall-through to the caller's error path.
fn bound_method_common_attr(
    function: &UserFunction,
    name: &str,
) -> Option<crate::error::Result<Value>> {
    match name {
        "__name__" => Some(Ok(Value::string(function.effective_name()))),
        "__qualname__" => Some(Ok(Value::string(function.effective_qualname()))),
        "__module__" => Some(Ok(function.module_value())),
        "__doc__" => Some(Ok(function.doc.borrow().clone())),
        "__dict__" => {
            let attrs_rc = func_attrs_rc(function);
            Some(Ok(attrs_rc.borrow().clone()))
        }
        "__annotations__" => Some(Ok(function.annotations_value())),
        "__defaults__" => {
            // #2395: positional defaults tuple (or per-object override), `None`
            // when none exist — CPython's `f.__defaults__` semantics.
            Some(Ok(function.defaults_value()))
        }
        "__kwdefaults__" => {
            // #2395: keyword-only defaults dict (or per-object override), `None`
            // when none exist — CPython's `f.__kwdefaults__` semantics.
            Some(Ok(function.kwdefaults_value()))
        }
        _ => {
            // Arbitrary dynamic attrs delegate to the underlying function.
            // Short-circuit without initialising if no attrs set yet.
            if let Some(rc) = function.attrs.borrow().as_ref().map(Rc::clone)
                && let Some(v) = rc
                    .borrow()
                    .as_dict()
                    .and_then(|d| d.get(&StrKey(name)).cloned())
            {
                return Some(Ok(v));
            }
            None
        }
    }
}
