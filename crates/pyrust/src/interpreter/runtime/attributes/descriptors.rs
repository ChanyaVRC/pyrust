/// Returns `true` if `val` is a data descriptor: it defines `__set__` or
/// `__delete__` on its type.  Data descriptors take priority over instance
/// `__dict__` in CPython's attribute lookup (PEP 3107 / Data Model §3.3.2).
///
/// For `property` (a BuiltinObject), it is always a data descriptor.
/// For user `PyInstance` values, we look up `__set__` or `__delete__` on the
/// instance's class.  Other value kinds are never descriptors.
pub(super) fn is_data_descriptor(val: &Value) -> bool {
    // property is always a data descriptor (has fget/fset/fdel slots).
    if pyrust_builtins::property::property_partial_slot(val) == Some(None) {
        return true;
    }
    // A `__slots__` member_descriptor is a data descriptor (it defines
    // __get__/__set__/__delete__), so it takes priority over instance storage
    // and reading an unset slot raises AttributeError (issue #2084).
    if pyrust_builtins::member_descriptor::as_member_descriptor(val).is_some() {
        return true;
    }
    if let ValueKind::PyInstance(inst) = val.kind() {
        let class = Rc::clone(&inst.borrow().class);
        return lookup_class_attr(&class, "__set__").is_some()
            || lookup_class_attr(&class, "__delete__").is_some();
    }
    false
}

/// Returns `true` if `val` is a non-data descriptor: defines `__get__` on
/// its type but NOT `__set__` or `__delete__`.  Non-data descriptors are
/// checked after instance `__dict__` in CPython's lookup order.
///
/// `cached_property` is already handled before this path so we don't need
/// to special-case it here — it would return `true` but is intercepted
/// earlier.
fn is_non_data_descriptor(val: &Value) -> bool {
    // The one-argument `super(cls)` is an unbound super object whose type
    // defines `tp_descr_get` but no `__set__`/`__delete__` — a non-data
    // descriptor.  Stored as a class attribute, it binds to the instance on
    // access (#2704), so it must flow through the generic descriptor protocol
    // rather than only via an explicit `__get__` call.
    if matches!(val.kind(), ValueKind::SuperProxyUnbound { .. }) {
        return true;
    }
    if let ValueKind::PyInstance(inst) = val.kind() {
        let class = Rc::clone(&inst.borrow().class);
        return lookup_class_attr(&class, "__get__").is_some()
            && lookup_class_attr(&class, "__set__").is_none()
            && lookup_class_attr(&class, "__delete__").is_none();
    }
    false
}

/// Call `descriptor.__get__(instance, owner)` and return the result.
///
/// Execute a directly-invoked property descriptor dunder bound via
/// `p.__get__` / `p.__set__` / `p.__delete__`.
///
/// `prop` is the property object; `kind` selects the dunder; `args` are the
/// call-site arguments.  The "no setter/deleter/getter" errors name the
/// property when it recorded one via `__set_name__` (issue #1846), else use
/// CPython's unnamed form (`property of '<owner>' object has no <which>`).
pub(crate) fn dispatch_property_method(
    interp: &mut Interpreter,
    prop: &Value,
    kind: pyrust_builtins::property::PropertyMethodKind,
    args: &[Value],
) -> Result<Value> {
    use pyrust_builtins::property::PropertyMethodKind as K;
    // CPython's slot wrappers validate arity (and raise TypeError) before the
    // missing-accessor AttributeError. __get__ takes obj + optional owner,
    // __set__ takes obj + value, __delete__ takes obj.
    match kind {
        K::Get => {
            if args.is_empty() || args.len() > 2 {
                return Err(pyrust_core::type_err!(
                    "expected 1 or 2 arguments, got {}",
                    args.len()
                ));
            }
            // Class-level access (`obj is None`) returns the property itself,
            // but only when an owner is supplied: CPython rejects
            // `__get__(None)` / `__get__(None, None)` with
            // `__get__(None, None) is invalid`.
            let obj = args[0].clone();
            if obj.is_none() {
                let owner = args.get(1).cloned().unwrap_or_else(Value::none);
                if owner.is_none() {
                    return Err(pyrust_core::type_err!("__get__(None, None) is invalid"));
                }
                return Ok(prop.clone());
            }
            let fget = pyrust_builtins::property::with_property(prop, |s| (*s.fget).clone())
                .unwrap_or_else(Value::none);
            if fget.is_none() {
                return Err(property_accessor_error(interp, prop, &obj, K::Get)?);
            }
            interp.call_function_expanded(
                fget,
                &[ExpandedCallArg {
                    name: None,
                    value: obj,
                }],
            )
        }
        K::Set => {
            if args.len() != 2 {
                return Err(pyrust_core::type_err!(
                    "expected 2 arguments, got {}",
                    args.len()
                ));
            }
            let obj = args[0].clone();
            let value = args[1].clone();
            let fset = pyrust_builtins::property::with_property(prop, |s| (*s.fset).clone())
                .unwrap_or_else(Value::none);
            if fset.is_none() {
                return Err(property_accessor_error(interp, prop, &obj, K::Set)?);
            }
            interp.call_function_expanded(
                fset,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: obj,
                    },
                    ExpandedCallArg { name: None, value },
                ],
            )
        }
        K::Delete => {
            if args.len() != 1 {
                return Err(pyrust_core::type_err!(
                    "expected 1 argument, got {}",
                    args.len()
                ));
            }
            let obj = args[0].clone();
            let fdel = pyrust_builtins::property::with_property(prop, |s| (*s.fdel).clone())
                .unwrap_or_else(Value::none);
            if fdel.is_none() {
                return Err(property_accessor_error(interp, prop, &obj, K::Delete)?);
            }
            interp.call_function_expanded(
                fdel,
                &[ExpandedCallArg {
                    name: None,
                    value: obj,
                }],
            )
        }
    }
}

/// CPython's property-accessor error:
/// `property '<name>' of '<owner>' object has no <which>`, or the unnamed
/// `property of '<owner>' object has no <which>` when the property was never
/// bound in a class body (no `__set_name__`; issue #1846).
fn property_accessor_error(
    interp: &mut Interpreter,
    prop: &Value,
    instance: &Value,
    kind: pyrust_builtins::property::PropertyMethodKind,
) -> Result<PyError> {
    let owner = value_type_name_str(instance);
    let name = pyrust_builtins::property::with_property(prop, |s| s.name.clone()).flatten();
    let prop_desc = property_description(interp, name.as_ref())?;
    let which = kind.accessor_name();
    Ok(pyrust_core::py_err!(
        "AttributeError",
        "{prop_desc} of '{owner}' object has no {which}"
    ))
}

/// Render the optional object retained by `property.__set_name__`.
///
/// CPython stores the name object verbatim and formats it with `%R` only when
/// an accessor error is raised. Keeping the rendering here, in the runtime
/// formatting domain, lets user-defined `__repr__` participate without making
/// `pyrust-builtins` depend on interpreter dispatch.
fn property_description(interp: &mut Interpreter, name: Option<&Value>) -> Result<String> {
    match name {
        Some(name) => Ok(format!(
            "{} {}",
            pyrust_builtins::property::TYPE_NAME,
            super::render_value_repr(interp, name)?
        )),
        None => Ok(pyrust_builtins::property::TYPE_NAME.to_string()),
    }
}

/// Read a `__slots__` slot value from an instance's storage (issue #2084).
/// `instance` must be a `PyInstance`; an unset slot raises AttributeError.
fn member_descriptor_get(
    instance: &Value,
    slot_id: pyrust_core::MemberSlotId,
    slot_name: &str,
) -> Result<Value> {
    if let ValueKind::PyInstance(inst) = instance.kind() {
        if let Some(v) = inst.borrow().attrs.get_member_slot(slot_id).cloned() {
            return Ok(v);
        }
        let class_name = inst.borrow().class.borrow().name.clone();
        return Err(pyrust_core::py_err!(
            "AttributeError",
            "'{class_name}' object has no attribute '{slot_name}'"
        ));
    }
    Ok(Value::none())
}
