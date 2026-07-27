/// Returns the singleton synthetic `object` class used as the terminal
/// entry of every class's `__mro__`. pyrust does not (yet) model `object`
/// as a real first-class type — every user class chains to `None` — so
/// this provides a stable, identity-comparable terminator so that
/// `A.__mro__[-1] is B.__mro__[-1]` holds, matching CPython.
pub(crate) fn object_class_singleton() -> Rc<RefCell<PyClass>> {
    OBJECT_CLASS.with(Rc::clone)
}

/// Returns the singleton `type` metaclass.  In CPython `type(int)` returns
/// `<class 'type'>` and `isinstance(int, type)` is `True` because every class
/// is an instance of `type` (the metaclass).  Using a per-thread singleton
/// mirrors the `object_class_singleton` pattern (issue #1312).
pub(crate) fn type_class_singleton() -> Rc<RefCell<PyClass>> {
    TYPE_CLASS.with(Rc::clone)
}

/// Returns the metaclass (metatype) of `class`.  A class with no explicit
/// metatype (the common case) is an instance of the built-in `type`
/// singleton, so this returns the `type` singleton in that case.
/// Issues #1955/#1956/#1960.
pub(crate) fn metaclass_of(class: &Rc<RefCell<PyClass>>) -> Rc<RefCell<PyClass>> {
    class
        .borrow()
        .metatype
        .clone()
        .unwrap_or_else(type_class_singleton)
}

/// Look up attribute `name` on the metaclass MRO of `class`, returning it only
/// when it is a *user* override — i.e. the metaclass is something other than
/// the built-in `type` singleton and the attribute is found before the walk
/// reaches `type`/`object`.  Returns `None` for ordinary classes (metatype is
/// `type`), so plain `Cls()` / `Cls.attr` / `isinstance` keep their fast
/// paths and never recurse into the default `type` slot.  Used for both
/// metaclass dunder hooks (`__call__` / `__instancecheck__` / `__getattr__`)
/// and plain metaclass attributes reached via `cls.attr`.
/// Issues #1955/#1956/#1960.
pub(crate) fn metaclass_dunder(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    let meta = class.borrow().metatype.clone()?;
    // A metatype that is the `type` singleton itself has no user override.
    if Rc::ptr_eq(&meta, &type_class_singleton()) {
        return None;
    }
    lookup_user_metaclass_attr(&meta, name)
}

/// Walk `meta`'s MRO looking for `name`, but stop short of the built-in
/// `type` and `object` singletons — those carry the *default* slots, which
/// must not be treated as user overrides (that would defeat the fast path
/// and risk infinite recursion in `type.__call__` chaining).
fn lookup_user_metaclass_attr(meta: &Rc<RefCell<PyClass>>, name: &str) -> Option<Value> {
    if Rc::ptr_eq(meta, &type_class_singleton()) || Rc::ptr_eq(meta, &object_class_singleton()) {
        return None;
    }
    let (value, base, extra_bases) = {
        let borrowed = meta.borrow();
        (
            borrowed.attrs.get(name).cloned(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if value.is_some() {
        return value;
    }
    if let Some(base) = base
        && let Some(v) = lookup_user_metaclass_attr(&base, name)
    {
        return Some(v);
    }
    for extra in &extra_bases {
        if let Some(v) = lookup_user_metaclass_attr(extra, name) {
            return Some(v);
        }
    }
    None
}

/// Dispatch a metaclass `__repr__` / `__str__` for a class *value* (issue
/// #2771).  `repr(cls)` / `str(cls)` must invoke `type(cls).__repr__(cls)` /
/// `type(cls).__str__(cls)` when the metaclass defines a user override —
/// CPython runs the type's metatype slot, not the default `<class '...'>`
/// format.
///
/// Returns `None` for an ordinary class (metatype is the built-in `type`
/// singleton, so `metaclass_dunder` finds nothing), letting the caller fall
/// through to the default class-repr format and keeping the common path free
/// of any interpreter dispatch.  `Some(Ok(s))` is the rendered string from the
/// metaclass method; `Some(Err(..))` propagates a non-string return or a raise
/// from inside the metaclass method.
///
/// `name` is `"__repr__"` or `"__str__"`.  For `__str__`, CPython's
/// `type.__str__` delegates to `type.__repr__`, so a metaclass that overrides
/// only `__repr__` still affects `str(cls)`; we mirror that by falling back to
/// the metaclass `__repr__` when `__str__` has no user override.
pub(crate) fn dispatch_metaclass_repr_str(
    interp: &mut Interpreter,
    class: &Rc<RefCell<PyClass>>,
    name: &str,
) -> Option<Result<String>> {
    let method_val = metaclass_dunder(class, name).or_else(|| {
        // `str(cls)` with no metaclass `__str__` falls back to the metaclass
        // `__repr__` (CPython: `type.__str__` calls `type.__repr__`).
        if name == "__str__" {
            metaclass_dunder(class, "__repr__")
        } else {
            None
        }
    })?;
    let cls_value = Value::py_class(Rc::clone(class));
    let result = match invoke_class_method(interp, method_val, cls_value, &[]) {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    Some(match result.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(pyrust_core::type_err!(
            "__{}__ returned non-string (type {})",
            if name == "__str__" { "str" } else { "repr" },
            pyrust_core::builtin_type_name(&result)
        )),
    })
}
