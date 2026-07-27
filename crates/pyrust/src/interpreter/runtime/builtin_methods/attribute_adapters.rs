// Attribute surfaces owned by concrete builtin callable/object adapters.

/// Bind class-level attributes whose behavior is specific to a concrete
/// built-in rather than to Python's generic descriptor protocol.
pub(crate) fn bind_builtin_class_special(
    class: &Rc<RefCell<PyClass>>,
    name: &str,
) -> Option<Value> {
    // Numeric-tower read-only properties and `conjugate` are concrete builtin
    // descriptors. Determine their owner from primitive singleton identity,
    // not from a Python-visible class name: `class int: pass` must not acquire
    // int's C descriptors, while bool and genuine int/float subclasses inherit
    // them from the canonical base.
    if matches!(
        name,
        "real" | "imag" | "numerator" | "denominator" | "conjugate"
    ) {
        let owner = c3_linearize_classes(class)
            .into_iter()
            .filter_map(|candidate| primitive_class_kind(&candidate))
            .find(|kind| matches!(kind, PrimitiveClassKind::Int | PrimitiveClassKind::Float));
        return match (owner, name) {
            (Some(PrimitiveClassKind::Int), "real") => Some(
                pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                    "real",
                    PrimitiveClassKind::Int.canonical_name(),
                ),
            ),
            (Some(PrimitiveClassKind::Int), "imag") => Some(
                pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                    "imag",
                    PrimitiveClassKind::Int.canonical_name(),
                ),
            ),
            (Some(PrimitiveClassKind::Int), "numerator") => Some(
                pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                    "numerator",
                    PrimitiveClassKind::Int.canonical_name(),
                ),
            ),
            (Some(PrimitiveClassKind::Int), "denominator") => Some(
                pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                    "denominator",
                    PrimitiveClassKind::Int.canonical_name(),
                ),
            ),
            (Some(PrimitiveClassKind::Int), "conjugate") => Some(
                pyrust_builtins::numeric_attrs_descriptor::method_descriptor(
                    "conjugate",
                    PrimitiveClassKind::Int.canonical_name(),
                ),
            ),
            (Some(PrimitiveClassKind::Float), "real") => Some(
                pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                    "real",
                    PrimitiveClassKind::Float.canonical_name(),
                ),
            ),
            (Some(PrimitiveClassKind::Float), "imag") => Some(
                pyrust_builtins::numeric_attrs_descriptor::getset_descriptor(
                    "imag",
                    PrimitiveClassKind::Float.canonical_name(),
                ),
            ),
            (Some(PrimitiveClassKind::Float), "conjugate") => Some(
                pyrust_builtins::numeric_attrs_descriptor::method_descriptor(
                    "conjugate",
                    PrimitiveClassKind::Float.canonical_name(),
                ),
            ),
            _ => None,
        };
    }
    None
}

/// Materialise a fresh bound builtin from a native-classmethod cache plan.
///
/// Descriptor representation remains private to the builtins boundary; the
/// attribute fast path consumes only this typed operation.
pub(crate) fn bind_cached_native_class_method(
    plan: &NativeClassMethodCachePlan,
    class: Rc<RefCell<PyClass>>,
) -> Option<Value> {
    pyrust_builtins::classmethod::bind_cached_native_class_method(plan, class)
}

/// Bind only descriptor categories explicitly installed by a built-in class.
///
/// Primitive values use specialised attribute adapters rather than the
/// `PyInstance` path, but classmethod/staticmethod semantics still come from
/// their canonical class dictionaries. Returning `None` for ordinary values
/// keeps concrete instance-method routing unchanged.
fn bind_explicit_builtin_descriptor(
    value: &Value,
    class: &Rc<RefCell<PyClass>>,
) -> Result<Option<Value>> {
    if let Some(binding) = pyrust_builtins::classmethod::as_class_method_any(value) {
        return pyrust_builtins::classmethod::bind_wrapped_class_method(binding, Rc::clone(class))
            .map(Some);
    }
    Ok(pyrust_builtins::classmethod::as_static_method_any(value))
}

/// Apply a cached `BuiltinFunction` class attribute to an instance.
///
/// `Some(value)` is a cache-safe result. `None` asks the fast path to fall
/// through to generic lookup so it can raise the full descriptor receiver
/// error. Registry-key spelling is interpreted only by
/// [`builtin_callable_metadata`], never by the cache.
pub(crate) fn bind_builtin_attribute(
    function: Value,
    instance: Rc<RefCell<PyInstance>>,
) -> Option<Value> {
    let registry_key = match function.kind() {
        ValueKind::BuiltinFunction(registry_key) => Some(registry_key),
        _ => None,
    };
    let Some(registry_key) = registry_key else {
        return Some(function);
    };
    let metadata = builtin_callable_metadata(registry_key);
    if metadata.kind != crate::builtin_registry::BuiltinCallableKind::MethodDescriptor {
        return Some(function);
    }
    if let Some(owner_tag) = metadata.descriptor_owner_tag() {
        let actual_class = Rc::clone(&instance.borrow().class);
        let owner = canonical_class_by_tag(owner_tag);
        if !class_is_subclass_of(&actual_class, &owner) {
            return None;
        }
    }
    Some(pyrust_builtins::bound_method::bound_method(
        metadata.python_name().to_string(),
        Value::py_instance(instance),
    ))
}

/// Adapt a method exposed by a stdlib subclass that re-owns selected
/// primitive descriptors.
///
/// PyRust implements `collections.OrderedDict` in Python, while CPython
/// exposes several of its methods as C `method_descriptor`s owned by the
/// `OrderedDict` class. Keep that concrete compatibility table in the builtin
/// adapter domain so generic class-attribute routing only consumes the typed
/// decision.
pub(crate) fn adapt_builtin_subclass_method(
    class: &Rc<RefCell<PyClass>>,
    name: &str,
    value: Value,
) -> Value {
    if is_primitive_class(class) {
        return value;
    }
    let Some(owner) = ordered_dict_owner(class) else {
        return value;
    };
    let owns = matches!(
        name,
        "clear"
            | "pop"
            | "popitem"
            | "update"
            | "setdefault"
            | "copy"
            | "keys"
            | "values"
            | "items"
            | "move_to_end"
    );
    if !owns {
        return value;
    }
    pyrust_builtins::unbound_method_descriptor::unbound_method_descriptor(
        Value::py_class(owner),
        name.to_string(),
        value,
    )
}

/// Resolve the semantic callable category and Python presentation for a
/// `BuiltinFunction`.
///
/// Registered callables carry authoritative metadata emitted by
/// `pyrust-derive`.  A small number of interpreter-owned synthetic descriptors
/// (notably generator methods) are intentionally not registry dispatch
/// entries; their legacy dotted representation is interpreted only here, at
/// the concrete builtin adapter boundary.  Generic attribute/cache code must
/// consume this typed result rather than infer semantics from punctuation in a
/// dispatch key.
pub(crate) fn builtin_callable_metadata(
    name: &'static str,
) -> crate::builtin_registry::BuiltinCallableMetadata {
    crate::builtin_registry::lookup_metadata(name).unwrap_or_else(|| {
        let Some((owner_qualname, _)) = name.rsplit_once('.') else {
            return crate::builtin_registry::BuiltinCallableMetadata::module_function(
                "builtins", name,
            );
        };
        let owner = owner_qualname.rsplit('.').next().unwrap_or(owner_qualname);
        let canonical_owner = if owner == "object" {
            Some(pyrust_core::CanonicalClassTag::Object)
        } else {
            pyrust_core::CanonicalClassTag::from_primitive_name(owner)
        };
        match canonical_owner {
            Some(owner) => {
                crate::builtin_registry::BuiltinCallableMetadata::canonical_method_descriptor(
                    "builtins", name, owner,
                )
            }
            None => crate::builtin_registry::BuiltinCallableMetadata::method_descriptor(
                "builtins", name,
            ),
        }
    })
}

impl Interpreter {
    pub(super) fn get_property_attribute(&mut self, target: &Value, name: &str) -> Result<Value> {
        let (fget_val, fset_val, fdel_val, doc_val) =
            pyrust_builtins::property::with_property(target, |s| {
                (
                    (*s.fget).clone(),
                    (*s.fset).clone(),
                    (*s.fdel).clone(),
                    s.doc.clone(),
                )
            })
            .expect("guard checked above");
        match name {
            "setter" => Ok(pyrust_builtins::property::property_setter_partial(
                fget_val, fdel_val,
            )),
            "deleter" => Ok(pyrust_builtins::property::property_deleter_partial(
                fget_val, fset_val,
            )),
            "getter" => Ok(pyrust_builtins::property::property_getter_partial(
                fset_val, fdel_val,
            )),
            "fget" => Ok(fget_val),
            "fset" => Ok(fset_val),
            "fdel" => Ok(fdel_val),
            // `property.__doc__` is the explicit `doc=` argument if one
            // was given, otherwise the getter's docstring (CPython
            // copies `fget.__doc__` into the property).  `None` when
            // neither is available (issue #1961).
            "__doc__" => Ok(match doc_val {
                Some(d) => d,
                None => match fget_val.kind() {
                    ValueKind::UserFunction(func) => func.doc.borrow().clone(),
                    _ => Value::none(),
                },
            }),
            // Descriptor-protocol dunders.  Accessing `p.__get__` etc.
            // yields a bound method-wrapper (so `hasattr(p, "__get__")`
            // is True and `f = p.__get__; f(obj, owner)` works); the
            // actual dispatch happens when the wrapper is called (see
            // `calls.rs`).
            "__get__" => Ok(pyrust_builtins::property::property_method(
                target.clone(),
                pyrust_builtins::property::PropertyMethodKind::Get,
            )),
            "__set__" => Ok(pyrust_builtins::property::property_method(
                target.clone(),
                pyrust_builtins::property::PropertyMethodKind::Set,
            )),
            "__delete__" => Ok(pyrust_builtins::property::property_method(
                target.clone(),
                pyrust_builtins::property::PropertyMethodKind::Delete,
            )),
            "__set_name__" => Ok(pyrust_builtins::bound_method::bound_method(
                "__set_name__",
                target.clone(),
            )),
            _ => Err(pyrust_core::py_err!(
                "AttributeError",
                "property object has no attribute '{name}'"
            )),
        }
    }

    pub(super) fn get_builtin_function_attribute(
        &mut self,
        target: &Value,
        func_name: &'static str,
        name: &str,
    ) -> Result<Value> {
        // __name__ / __qualname__ / __module__ on builtin functions.
        //
        let metadata = builtin_callable_metadata(func_name);
        if name == "__name__" {
            return Ok(Value::string(metadata.python_name()));
        }
        if name == "__qualname__" {
            return Ok(Value::string(metadata.python_qualname()));
        }
        if name == "__module__" {
            if metadata.kind == crate::builtin_registry::BuiltinCallableKind::MethodDescriptor {
                return Err(pyrust_core::py_err!(
                    "AttributeError",
                    "'method_descriptor' object has no attribute '__module__'"
                ));
            }
            let function = target
                .as_function_rc()
                .expect("BuiltinFunction must carry Rc<UserFunction>");
            return Ok(function.module_value_with_default(metadata.python_module()));
        }
        if name == "__call__" && !matches!(func_name, "generator" | "str" | "property") {
            // Issue #2550: builtin functions (`len`, `print`) and method
            // descriptors (`str.upper`) are callable, so CPython exposes
            // `len.__call__ == <method-wrapper '__call__' of
            // builtin_function_or_method object at 0x...>` (and
            // `... of method_descriptor object ...` for the dotted form),
            // with `hasattr(len, '__call__') is True`.  Surface a wrapper
            // bound to the builtin; calling it re-dispatches onto the
            // builtin (so `len.__call__([1,2,3]) == 3`).  The `generator`
            // / `str` / `property` type-token names are excluded — they
            // are handled as type objects below, not as plain builtins.
            let owner = match metadata.kind {
                crate::builtin_registry::BuiltinCallableKind::ModuleFunction => {
                    "builtin_function_or_method"
                }
                crate::builtin_registry::BuiltinCallableKind::MethodDescriptor => {
                    "method_descriptor"
                }
            };
            return Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                target.clone(),
                owner,
            ));
        }
        if func_name == "generator" {
            // Issue #1413: type(gen).__iter__ and type(gen).__next__.
            // CPython exposes these as slot wrappers on the generator
            // type.  Return unbound BuiltinFunction descriptors so that
            // hasattr(type(gen), '__iter__') is True and calling
            // type(gen).__iter__(g) works via call_function_expanded.
            match name {
                "__iter__" => return Ok(Value::builtin_function("generator.__iter__")),
                "__next__" => return Ok(Value::builtin_function("generator.__next__")),
                "send" => return Ok(Value::builtin_function("generator.send")),
                "close" => return Ok(Value::builtin_function("generator.close")),
                "throw" => return Ok(Value::builtin_function("generator.throw")),
                _ => {}
            }
        }
        if func_name == "str" {
            match name {
                "lower" => Ok(Value::builtin_function("str.lower")),
                "upper" => Ok(Value::builtin_function("str.upper")),
                "strip" => Ok(Value::builtin_function("str.strip")),
                "lstrip" => Ok(Value::builtin_function("str.lstrip")),
                "rstrip" => Ok(Value::builtin_function("str.rstrip")),
                "capitalize" => Ok(Value::builtin_function("str.capitalize")),
                "split" => Ok(Value::builtin_function("str.split")),
                "join" => Ok(Value::builtin_function("str.join")),
                "replace" => Ok(Value::builtin_function("str.replace")),
                "find" => Ok(Value::builtin_function("str.find")),
                "rfind" => Ok(Value::builtin_function("str.rfind")),
                "index" => Ok(Value::builtin_function("str.index")),
                "rindex" => Ok(Value::builtin_function("str.rindex")),
                "count" => Ok(Value::builtin_function("str.count")),
                "startswith" => Ok(Value::builtin_function("str.startswith")),
                "endswith" => Ok(Value::builtin_function("str.endswith")),
                "format" => Ok(Value::builtin_function("str.format")),
                "format_map" => Ok(Value::builtin_function("str.format_map")),
                "isdigit" => Ok(Value::builtin_function("str.isdigit")),
                "isalpha" => Ok(Value::builtin_function("str.isalpha")),
                "isalnum" => Ok(Value::builtin_function("str.isalnum")),
                "isspace" => Ok(Value::builtin_function("str.isspace")),
                _ => Err(pyrust_core::py_err!(
                    "AttributeError",
                    "type object 'str' has no attribute '{name}'"
                )),
            }
        } else if func_name == "property" && matches!(name, "__get__" | "__set__" | "__delete__") {
            // Issue #1835: the `property` type token (a BuiltinFunction
            // in pyrust, since `property` is not yet a real PyClass)
            // exposes the descriptor protocol so that
            // `hasattr(property, "__get__")` is True like CPython.  The
            // wrapper is bound to an empty property; this serves
            // introspection (`hasattr`/`getattr`) — calling it unbound
            // from the type object is not a supported path.
            use pyrust_builtins::property::PropertyMethodKind as K;
            let kind = match name {
                "__get__" => K::Get,
                "__set__" => K::Set,
                _ => K::Delete,
            };
            let empty =
                pyrust_builtins::property::property(Value::none(), Value::none(), Value::none());
            Ok(pyrust_builtins::property::property_method(empty, kind))
        } else {
            Err(pyrust_core::py_err!(
                "AttributeError",
                "type object '{}' has no attribute '{name}'",
                func_name
            ))
        }
    }

    pub(super) fn get_builtin_object_attribute(
        &mut self,
        target: &Value,
        name: &str,
    ) -> Result<Value> {
        if let Some(class) = crate::interpreter::primitive_class_for_value(target)
            && let Some(descriptor) = lookup_class_attr(&class, name)
            && let Some(bound) = bind_explicit_builtin_descriptor(&descriptor, &class)?
        {
            return Ok(bound);
        }

        // Builtin bound methods (BuiltinObject with BoundMethodState):
        // expose __name__, __qualname__, __self__, __module__, __doc__
        // to match CPython's builtin_function_or_method attributes.
        //
        // Kept in its own arm so the as_bound_method check is only
        // reached for BuiltinObject values — not for List/Dict/Set/etc.
        // (which previously fell through to _ and paid the check cost
        // on every method lookup like lst.append).
        if let Some((method_name, receiver)) =
            pyrust_builtins::bound_method::as_bound_method(target)
        {
            let is_wrapper = pyrust_builtins::bound_method::is_method_wrapper(target);
            match name {
                "__name__" => return Ok(Value::string(method_name.as_str())),
                "__qualname__" => {
                    let type_name = pyrust_core::builtin_type_name(&receiver);
                    return Ok(Value::string(format!("{type_name}.{method_name}")));
                }
                "__self__" => return Ok(receiver),
                "__module__" if !is_wrapper => {
                    return Ok(pyrust_builtins::bound_method::module_value(target)
                        .expect("captured bound method must carry module state"));
                }
                "__module__" => {
                    return Err(pyrust_core::py_err!(
                        "AttributeError",
                        "'method-wrapper' object has no attribute '__module__'"
                    ));
                }
                "__doc__" => return Ok(Value::none()),
                "__call__" => {
                    // Issue #2550: a builtin bound method (`[].append`) is
                    // callable; CPython's `l.append.__call__` is a
                    // `builtin_function_or_method object` method-wrapper.
                    // Re-dispatches onto the bound method.
                    return Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                        target.clone(),
                        "builtin_function_or_method",
                    ));
                }
                _ => {}
            }
        }
        if name == "__call__"
            && pyrust_builtins::native_builtin_callable::as_native_static_builtin(target).is_some()
        {
            return Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                target.clone(),
                "builtin_function_or_method",
            ));
        }
        // Non-bound-method BuiltinObjects (GenericAlias, frozenset,
        // dict views, file, enumerate, zip, reversed, chain,
        // cached_property, …) also reach this arm.
        // First probe the type's custom `getattr` (e.g. GenericAlias
        // exposes `__origin__` and `__args__` this way), then fall
        // back to builtin method lookup.
        if let ValueKind::BuiltinObject { ops, state } = target.kind() {
            if let Some(val) = ops.getattr(state, name) {
                return Ok(val);
            }
            // Issue #2550: a `__call__` method-wrapper is itself
            // callable, so CPython exposes `f.__call__.__call__ ==
            // <method-wrapper '__call__' of method-wrapper object at
            // 0x...>` (and `hasattr(f.__call__, '__call__') is True`),
            // with calling it re-dispatching onto the underlying
            // callable.  Surface another wrapper bound to this one.
            if name == "__call__"
                && pyrust_builtins::type_call_wrapper::is_type_call_wrapper(target)
            {
                return Ok(pyrust_builtins::type_call_wrapper::call_wrapper(
                    target.clone(),
                    "method-wrapper",
                ));
            }
            // `__slots__` member_descriptor exposes its descriptor
            // protocol as bound methods (`S.x.__get__`, …), issue #2084.
            if pyrust_builtins::member_descriptor::is_member_descriptor(target)
                && matches!(name, "__get__" | "__set__" | "__delete__")
            {
                return Ok(pyrust_builtins::bound_method::bound_method(
                    name,
                    target.clone(),
                ));
            }
            // Issue #2133: a `GenericAlias` (`list[int]`) proxies
            // attribute access to its `__origin__`, so `list[int].
            // __name__ == 'list'`, `.__mro__`, instance methods, etc.
            // resolve to the origin's.  CPython's `ga_getattro` keeps a
            // small reserved set on the alias itself (`__origin__`,
            // `__args__`, `__parameters__` are served by `ops.getattr`
            // above; `__class__` resolves to the GenericAlias type, not
            // the origin; `__mro_entries__`/`__reduce__`/`__reduce_ex__`/
            // `__copy__`/`__deepcopy__` are the alias's own protocol
            // methods) and forwards everything else to the origin.
            if pyrust_builtins::generic_alias::is_generic_alias(target)
                && !matches!(
                    name,
                    "__class__"
                        | "__mro_entries__"
                        | "__reduce__"
                        | "__reduce_ex__"
                        | "__copy__"
                        | "__deepcopy__"
                )
                && let Some(origin) = ops.getattr(state, "__origin__")
            {
                return self.get_attr(&origin, name);
            }
        }
        if builtin_has_method(target, name) {
            return Ok(pyrust_builtins::bound_method::bound_method(
                name,
                target.clone(),
            ));
        }
        // Issue #2151: BuiltinObject primitives (frozenset, bytearray,
        // mappingproxy) inherit `object`'s dunders (`__eq__`, `__str__`,
        // `__hash__`, …) via their primitive class, the same as the
        // scalar/container `_` arm below.  Without this fall-through
        // `hasattr(frozenset(), '__eq__')` was False while
        // `dir(frozenset())` listed it.
        if let Some(cls) = crate::interpreter::primitive_class_for_value(target)
            && let Some(val) = lookup_class_attr(&cls, name)
        {
            return Ok(val);
        }
        let type_name = pyrust_core::builtin_type_name(target);
        Err(PyError::attribute_error(
            format!("'{type_name}' object has no attribute '{name}'"),
            Some(name.to_string()),
            Some(target.clone()),
        ))
    }

    pub(super) fn get_builtin_value_attribute(
        &mut self,
        target: &Value,
        name: &str,
    ) -> Result<Value> {
        if let Some(value) = pyrust_builtins::numeric_attrs_descriptor::complex_attr(target, name)
            .or_else(|| pyrust_builtins::numeric_attrs_descriptor::numeric_tower_attr(target, name))
            .or_else(|| pyrust_builtins::numeric_attrs_descriptor::range_attr(target, name))
        {
            return Ok(value);
        }
        // Ordinary native instance methods are immutable table entries. Probe
        // their typed inventory before walking the primitive class MRO: the
        // latter is needed only for explicitly installed classmethod/
        // staticmethod descriptors and inherited object attributes.
        if builtin_has_method(target, name) {
            return Ok(pyrust_builtins::bound_method::bound_method(
                name,
                target.clone(),
            ));
        }
        if let Some(class) = crate::interpreter::primitive_class_for_value(target)
            && let Some(descriptor) = lookup_class_attr(&class, name)
            && let Some(bound) = bind_explicit_builtin_descriptor(&descriptor, &class)?
        {
            return Ok(bound);
        }
        if name == "__hash__"
            && matches!(
                pyrust_core::builtin_type_name(target).as_ref(),
                "list" | "dict" | "set" | "bytearray"
            )
        {
            return Ok(Value::none());
        }
        if let Some(class) = crate::interpreter::primitive_class_for_value(target)
            && let Some(value) = lookup_class_attr(&class, name)
        {
            return Ok(value);
        }
        let type_name = pyrust_core::builtin_type_name(target);
        Err(PyError::attribute_error(
            format!("'{type_name}' object has no attribute '{name}'"),
            Some(name.to_string()),
            Some(target.clone()),
        ))
    }
}
/// Returns `true` if `name` is a built-in method on `target`'s type.
/// Used by `get_attr` to produce `BuiltinBoundMethod` values.
pub(super) fn builtin_has_method(target: &Value, name: &str) -> bool {
    // Issue #1909: container/sequence protocol dunders (`__len__`,
    // `__getitem__`, `__contains__`, `__add__`, …) are advertised by
    // `dir`/`hasattr` and resolvable as bound method-wrappers.  Gated on the
    // `__` prefix so the common method-name lookup (`lst.append`, `s.upper`)
    // pays only a cheap byte comparison before the per-type table below.
    if name.starts_with("__") {
        let type_name = pyrust_core::builtin_type_name(target);
        if is_protocol_dunder(&type_name, name) {
            return true;
        }
        // Issue #2151: object-protocol methods every built-in data value
        // inherits from `object` (`__sizeof__`/`__dir__`/`__reduce__`/
        // `__reduce_ex__`), plus `None.__bool__`.  Resolved as bound
        // method-wrappers so `(5).__sizeof__()` / `None.__bool__()` work;
        // dispatched in `bound_method_dispatch_inner`.  Gated to built-in
        // data values so function/class/bound-method attribute access is
        // untouched.
        if crate::interpreter::primitive_class_for_value(target).is_some()
            && is_object_protocol_method(target, name)
        {
            return true;
        }
        // Issue #2191: `(5).__format__(spec)` / `"hi".__format__(spec)` etc. must
        // route through the real format machinery (same result as
        // `format(self, spec)`).  Expose `__format__` as a bound method-wrapper
        // on every built-in data value so the dispatch lands in
        // `bound_method_dispatch_inner` (which delegates to `apply_format_spec`)
        // rather than the unbound `object.__format__` class-attr that drops
        // `self` and returns the spec verbatim.
        if name == "__format__" && crate::interpreter::primitive_class_for_value(target).is_some() {
            return true;
        }
    }
    match target.kind() {
        // bool is a subclass of int; hasattr(True, "bit_length") must return True.
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_) => {
            pyrust_builtins::int::has_method(name)
        }
        ValueKind::Float(_) => pyrust_builtins::float::has_method(name),
        ValueKind::Complex(_, _) => pyrust_builtins::complex::has_method(name),
        ValueKind::Bytes(_) => pyrust_builtins::bytes::has_method(name),
        ValueKind::Str(_) => pyrust_builtins::string::has_method(name),
        ValueKind::List(_) => pyrust_builtins::list::has_method(name),
        ValueKind::Tuple(_) => pyrust_builtins::tuple::has_method(name),
        ValueKind::Dict(_) => pyrust_builtins::dict::has_method(name),
        ValueKind::Set(_) => pyrust_builtins::set::has_method(name),
        ValueKind::Range { .. } | ValueKind::BigRange { .. } => {
            matches!(name, "__iter__" | "__len__" | "count" | "index")
        }
        ValueKind::BuiltinObject { ops, .. } => ops.has_method(name),
        _ => false,
    }
}
