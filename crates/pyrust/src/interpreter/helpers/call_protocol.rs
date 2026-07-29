pub(crate) struct PrintOptions {
    pub(crate) values: Vec<Value>,
    pub(crate) sep: String,
    pub(crate) end: String,
    /// `None` means write to stdout; `Some(v)` means call `v.write(...)`.
    pub(crate) file: Option<Value>,
    /// When true and `file` is `Some`, call `file.flush()` after writing.
    pub(crate) flush: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ExpandedCallArg {
    pub(crate) name: Option<String>,
    pub(crate) value: Value,
}

/// Inline-4 buffer for building small call-arg slices without heap allocation.
/// Covers `self + 0..3 args` — the dominant case for method invocations.
pub(crate) type ExpandedArgBuf = smallvec::SmallVec<[ExpandedCallArg; 4]>;

const CANONICAL_DESCRIPTOR_OWNER_TAGS: [pyrust_core::CanonicalClassTag; 17] = [
    pyrust_core::CanonicalClassTag::Object,
    pyrust_core::CanonicalClassTag::Bool,
    pyrust_core::CanonicalClassTag::Bytearray,
    pyrust_core::CanonicalClassTag::Bytes,
    pyrust_core::CanonicalClassTag::Complex,
    pyrust_core::CanonicalClassTag::Dict,
    pyrust_core::CanonicalClassTag::Ellipsis,
    pyrust_core::CanonicalClassTag::Float,
    pyrust_core::CanonicalClassTag::Frozenset,
    pyrust_core::CanonicalClassTag::Int,
    pyrust_core::CanonicalClassTag::List,
    pyrust_core::CanonicalClassTag::MappingProxy,
    pyrust_core::CanonicalClassTag::NoneType,
    pyrust_core::CanonicalClassTag::NotImplementedType,
    pyrust_core::CanonicalClassTag::Set,
    pyrust_core::CanonicalClassTag::Str,
    pyrust_core::CanonicalClassTag::Tuple,
];

thread_local! {
    /// Unregistered protocol sentinels are immutable. Resolve their canonical
    /// owner by Value identity once, then keep the typed result for subsequent
    /// dispatches instead of rescanning every canonical class attribute map.
    static UNREGISTERED_DESCRIPTOR_OWNER_CACHE:
        RefCell<HashMap<&'static str, Option<pyrust_core::CanonicalClassTag>>> =
        RefCell::new(HashMap::new());
}

#[derive(Clone, Copy)]
struct BuiltinMethodResolution {
    registration: Option<&'static crate::builtin_registry::BuiltinReg>,
    canonical_owner: Option<pyrust_core::CanonicalClassTag>,
}

/// Resolve a builtin descriptor's immutable canonical owner without deriving
/// semantics from its qualified display/registry name.
///
/// Registered descriptors carry a typed tag. Protocol sentinels without a
/// registry body (notably `bytes.__iter__`) fall back to Value-identity
/// matching against canonical classes' own attribute dictionaries.
fn canonical_builtin_descriptor_owner_tag(
    method: &Value,
    registration: Option<&'static crate::builtin_registry::BuiltinReg>,
) -> Option<pyrust_core::CanonicalClassTag> {
    let ValueKind::BuiltinFunction(registry_key) = method.kind() else {
        return None;
    };
    if let Some(owner) = registration.and_then(|entry| entry.metadata.descriptor_owner_tag()) {
        return Some(owner);
    }

    if let Some(cached) =
        UNREGISTERED_DESCRIPTOR_OWNER_CACHE.with(|cache| cache.borrow().get(registry_key).copied())
    {
        return cached;
    }
    let resolved = CANONICAL_DESCRIPTOR_OWNER_TAGS
        .iter()
        .copied()
        .find(|&tag| {
            let owner = canonical_class_by_tag(tag);
            owner
                .borrow()
                .attrs
                .values()
                .any(|owned| values_are_identical(owned, method))
        });
    UNREGISTERED_DESCRIPTOR_OWNER_CACHE.with(|cache| {
        cache.borrow_mut().insert(registry_key, resolved);
    });
    resolved
}

fn canonical_descriptor_method_name(
    method: &Value,
    owner_tag: pyrust_core::CanonicalClassTag,
    registration: Option<&'static crate::builtin_registry::BuiltinReg>,
) -> String {
    if let Some(entry) = registration {
        return entry.metadata.python_name().to_string();
    }
    let owner = canonical_class_by_tag(owner_tag);
    let borrowed = owner.borrow();
    borrowed
        .attrs
        .iter()
        .find(|(_, owned)| values_are_identical(owned, method))
        .map_or_else(|| "<unknown>".to_string(), |(name, _)| name.clone())
}

fn canonical_descriptor_uses_method_wording(
    owner: pyrust_core::CanonicalClassTag,
    method: &str,
) -> bool {
    if !(method.starts_with("__") && method.ends_with("__")) {
        return true;
    }
    matches!(
        (owner, method),
        (
            pyrust_core::CanonicalClassTag::List | pyrust_core::CanonicalClassTag::Dict,
            "__getitem__" | "__reversed__"
        ) | (
            pyrust_core::CanonicalClassTag::Dict
                | pyrust_core::CanonicalClassTag::Set
                | pyrust_core::CanonicalClassTag::Frozenset,
            "__contains__"
        ) | (
            pyrust_core::CanonicalClassTag::Int
                | pyrust_core::CanonicalClassTag::Bool
                | pyrust_core::CanonicalClassTag::Float,
            "__round__" | "__trunc__" | "__floor__" | "__ceil__"
        ) | (
            pyrust_core::CanonicalClassTag::Object,
            "__reduce__" | "__reduce_ex__" | "__sizeof__" | "__dir__" | "__format__"
        )
    )
}

fn resolve_and_validate_builtin_method(
    method: &Value,
    instance: &Value,
) -> Result<BuiltinMethodResolution> {
    let registration = match method.kind() {
        ValueKind::BuiltinFunction(registry_key) => {
            crate::builtin_registry::lookup_registration(registry_key)
        }
        _ => None,
    };
    let canonical_owner = canonical_builtin_descriptor_owner_tag(method, registration);
    let resolution = BuiltinMethodResolution {
        registration,
        canonical_owner,
    };
    let Some(owner_tag) = canonical_owner else {
        return Ok(resolution);
    };
    let owner = canonical_class_by_tag(owner_tag);
    let actual_class = match instance.kind() {
        ValueKind::PyInstance(object) => Some(Rc::clone(&object.borrow().class)),
        ValueKind::PyClass(class) => Some(Rc::clone(class)),
        _ => match value_class(instance).kind() {
            ValueKind::PyClass(class) => Some(Rc::clone(class)),
            _ => None,
        },
    };
    if actual_class
        .as_ref()
        .is_some_and(|actual| class_is_subclass_of(actual, &owner))
    {
        return Ok(resolution);
    }

    let method_name = canonical_descriptor_method_name(method, owner_tag, registration);
    let owner_name = owner_tag.canonical_name();
    let actual_name = value_type_name_str(instance);
    if canonical_descriptor_uses_method_wording(owner_tag, &method_name) {
        Err(pyrust_core::descriptor_requires!(
            method_name,
            owner_name,
            actual_name,
            method
        ))
    } else {
        Err(pyrust_core::descriptor_requires!(
            method_name,
            owner_name,
            actual_name
        ))
    }
}

/// Invoke a method that was looked up on a class — handling both
/// `UserFunction` methods (compiled Python bytecode, bound via the
/// interpreter's user-function path) and `BuiltinFunction` methods
/// (registered Rust dispatch fns from `pyrust_module!`'s `class` block).
///
/// In both cases `instance` is prepended as the implicit `self` —
/// matching how `inst.method(...)` semantics work in CPython.  This
/// helper centralises the binding rule so dunder dispatch sites
/// (`__getitem__`, `__iter__`, `__call__`, `__len__`, `__init__`,
/// …) don't have to repeat the UserFunction-vs-BuiltinFunction
/// branching at every call site.
///
/// The receiver is *not* prepended when the resolved slot carries its own
/// descriptor semantics: a `staticmethod` slot is called with `args` alone and
/// a `classmethod` slot receives the owning class instead.  Both spellings —
/// the `UserFunction` kind tag that `@staticmethod` / `@classmethod` over a
/// Python function produces, and the `BuiltinObject` wrappers used for every
/// other wrapped value — are honoured here so no dunder dispatch site has to
/// know the difference (issue #2939).
pub(crate) fn invoke_class_method(
    interp: &mut Interpreter,
    method_val: Value,
    instance: Value,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    // Non-UserFunction classmethod/staticmethod descriptors are explicit
    // wrappers owned by the class provider. Honour their binding semantics
    // before treating the resolved value as an ordinary callable slot.
    if let Some(wrapped) = pyrust_builtins::classmethod::as_class_method_any(&method_val) {
        let owner = match instance.kind() {
            ValueKind::PyClass(class) => Some(Rc::clone(class)),
            ValueKind::PyInstance(object) => Some(Rc::clone(&object.borrow().class)),
            _ => match value_class(&instance).kind() {
                ValueKind::PyClass(class) => Some(Rc::clone(class)),
                _ => None,
            },
        };
        if let Some(owner) = owner {
            let bound = pyrust_builtins::classmethod::bind_wrapped_class_method(wrapped, owner)?;
            return interp.call_function_expanded(bound, args);
        }
    }
    if let Some(wrapped) = pyrust_builtins::classmethod::as_static_method_any(&method_val) {
        return interp.call_function_expanded(wrapped, args);
    }

    match method_val.kind() {
        ValueKind::UserFunction(f) => {
            // `@classmethod` / `@staticmethod` over a Python function do NOT
            // produce a wrapper value in pyrust — they Rc-share the original
            // body and are distinguished only by `UserFunction::kind` (see
            // `UserFunctionKind`).  The `as_*_method_any` probes above therefore
            // only catch the *non*-function wrappers, and every implicit dunder
            // dispatch used to prepend the receiver regardless of the tag:
            // `__len__ = staticmethod(lambda: 3)` called the lambda with a
            // stray `self`, and a classmethod dunder received the instance
            // where CPython passes the class (issue #2939).
            //
            // CPython's `_PyObject_LookupSpecial` binds the type-level slot
            // through the descriptor protocol, so honour the same three
            // outcomes here.  `Regular` (the overwhelmingly common case) and
            // `Builtin` keep the previous receiver-prepending path and stay
            // first in the match, so the hot dunder dispatch pays only an
            // already-loaded enum-tag compare.
            match f.kind {
                pyrust_core::UserFunctionKind::StaticMethod => {
                    // `staticmethod.__get__` yields the wrapped callable
                    // untouched — no receiver is passed.
                    let unwrapped = match f.wrapped_func.as_ref() {
                        Some(inner) => Value::user_function(Rc::clone(inner)),
                        None => Value::with_function_kind(
                            Rc::clone(f),
                            pyrust_core::UserFunctionKind::Regular,
                        ),
                    };
                    return interp.call_function_expanded(unwrapped, args);
                }
                pyrust_core::UserFunctionKind::ClassMethod => {
                    // `classmethod.__get__` binds the owning class in place of
                    // the receiver.  Resolve it exactly as the wrapped-
                    // classmethod branch above does, so both spellings of
                    // `@classmethod` agree on what `cls` is.
                    let owner = match instance.kind() {
                        ValueKind::PyClass(class) => Some(Rc::clone(class)),
                        ValueKind::PyInstance(object) => Some(Rc::clone(&object.borrow().class)),
                        _ => match value_class(&instance).kind() {
                            ValueKind::PyClass(class) => Some(Rc::clone(class)),
                            _ => None,
                        },
                    };
                    if let Some(owner) = owner {
                        let bound = Value::class_bound_method(Rc::clone(f), owner);
                        return interp.call_function_expanded(bound, args);
                    }
                }
                _ => {}
            }
            let func = Rc::clone(f);
            return interp.call_user_function_expanded(func, args, &[instance]);
        }
        ValueKind::BuiltinFunction(name) => {
            let resolution = resolve_and_validate_builtin_method(&method_val, &instance)?;
            // Issue #1909: container protocol-dunder sentinels
            // (`dict.__contains__`, `list.__setitem__`, …) registered on the
            // primitive class objects have no registry body — they dispatch
            // through the operator machinery.  Route them here (covering the
            // implicit `in` / `[]` operator dispatch on a primitive *subclass*
            // and `super().__contains__(...)` calls) before the registry probe.
            let canonical_protocol = resolution.canonical_owner.and_then(|owner| {
                let method: std::borrow::Cow<'static, str> =
                    if let Some(entry) = resolution.registration {
                        std::borrow::Cow::Borrowed(entry.metadata.python_name())
                    } else {
                        std::borrow::Cow::Owned(canonical_descriptor_method_name(
                            &method_val,
                            owner,
                            None,
                        ))
                    };
                (method.starts_with("__")
                    && builtin_methods::is_protocol_dunder(owner.canonical_name(), &method))
                .then_some(method)
            });
            let legacy_protocol = if resolution.canonical_owner.is_none() {
                builtin_methods::legacy_builtin_protocol_method(name).map(str::to_string)
            } else {
                None
            };
            if let Some(method) = canonical_protocol
                .map(std::borrow::Cow::into_owned)
                .or(legacy_protocol)
            {
                // Resolve the receiver to its backing primitive when the
                // instance is a builtin-subclass PyInstance; a plain
                // primitive (super() from a non-subclass) is used directly.
                let receiver = match instance.kind() {
                    ValueKind::PyInstance(inst) => {
                        instance_builtin_data(inst).unwrap_or_else(|| instance.clone())
                    }
                    _ => instance.clone(),
                };
                let rest: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                return interp.dispatch_builtin_protocol_dunder(&method, receiver, rest);
            }
            // PEP 654: BaseExceptionGroup.derive / subgroup / split are not
            // registry builtins (they need interpreter access for predicates and
            // a subclass's overridden `derive`).  Dispatch them here with the
            // receiver prepended.
            if matches!(
                name,
                "BaseExceptionGroup.derive"
                    | "BaseExceptionGroup.subgroup"
                    | "BaseExceptionGroup.split"
            ) {
                let mut combined: Vec<ExpandedCallArg> = Vec::with_capacity(args.len() + 1);
                combined.push(ExpandedCallArg {
                    name: None,
                    value: instance.clone(),
                });
                combined.extend(args.iter().cloned());
                return match name {
                    "BaseExceptionGroup.derive" => interp.exception_group_derive(&combined),
                    "BaseExceptionGroup.subgroup" => {
                        interp.exception_group_subgroup_or_split(&combined, false)
                    }
                    _ => interp.exception_group_subgroup_or_split(&combined, true),
                };
            }
            // Representation-substitutability boundary (#2386): an inherited
            // builtin method `"<type>.<method>"` that has no registry body and
            // is resolved on a builtin-subclass instance dispatches on the
            // instance's backing value.  Reaching here with a `BuiltinFunction`
            // sentinel means the subclass did NOT override the method (a user
            // override resolves to a `UserFunction` via `lookup_class_attr`), so
            // unwrapping is unconditional once the type matches.
            //
            // This is what makes ops-table types work uniformly: `bytearray`'s
            // methods (`upper`, `find`, `append`, `__iter__`, …) live on the
            // BuiltinObject ops table, never in the string-keyed registry, so
            // the registry probe below misses them (`internal: builtin method
            // 'bytearray.upper' not in registry`, #2324).  Re-dispatch through a
            // bound_method on the unwrapped backing — the same mechanism the
            // `super().<method>()` path uses — so every builtin type routes
            // through its own per-type `call`/ops with no per-type whitelist.
            // Gated on the registry MISS so registry-backed methods and
            // construction dunders (`__init__`/`__new__` resolved by
            // `call_class_expanded`) keep their existing dispatch.
            let dispatch = match resolution.registration {
                Some(entry) => entry.dispatch,
                None => {
                    if let Some(bound) =
                        builtin_methods::bind_legacy_builtin_subclass_backing(name, &instance)
                    {
                        return interp.call_function_expanded(bound, args);
                    }
                    return Err(PyError::Runtime(format!(
                        "internal: builtin method '{name}' not in registry"
                    )));
                }
            };
            // Reuse the interpreter-level buffer to eliminate a per-invocation
            // heap allocation on the hot dunder dispatch path.  `std::mem::take`
            // leaves an empty SmallVec in `interp.invoke_arg_buf`; on recursive
            // re-entry the field is already empty so a fresh SmallVec is used
            // only for the nested call.  The buffer is always restored after
            // dispatch (both Ok and Err paths).
            let mut combined = std::mem::take(&mut interp.invoke_arg_buf);
            combined.clear();
            combined.push(ExpandedCallArg {
                name: None,
                value: instance,
            });
            combined.extend(args.iter().cloned());
            let result = dispatch(interp, &combined);
            interp.invoke_arg_buf = combined;
            return result;
        }
        // The non-function arm is handled after the match so `method_val` can be
        // moved out of the borrow taken by `method_val.kind()` above.
        _ => {}
    }
    // Issue #2054: the resolved slot is not a plain function but may still be
    // callable — a bound method, a class object, or a callable *instance* (an
    // object whose class defines `__call__`).  CPython invokes whatever the slot
    // resolves to.  Such a slot is *not* a descriptor, so (unlike a function
    // slot) it does NOT receive the receiver as `self`: `__len__ = Caller()`
    // calls `Caller()()` with no implicit self, `__add__ = Caller()` calls
    // `Caller()(other)`.  Route through the normal call machinery with `args`.
    //
    // Genuinely non-callable slots (`Foo.__len__ = 5`) raise the standard
    // "object is not callable" keyed on the *resolved value's* type, not the
    // owning class: `len(D())` with `__len__ = 5` -> "'int' object is not
    // callable".  Match that exactly so every implicit-dunder dispatch path
    // agrees with CPython 3.12 (issue #1963 / #2055).
    if slot_is_callable(&method_val) {
        interp.call_function_expanded(method_val, args)
    } else {
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object is not callable",
                value_type_name_str(&method_val)
            ),
        ))
    }
}

/// If `value` is a `PyInstance` whose class exposes the mapping protocol,
/// visit its `(PyKey, Value)` pairs in source order.  CPython first materialises
/// the iterable returned by `keys()`, then resolves and visits each key through
/// `__getitem__` one at a time.  A receiver such as `dict.update` can therefore
/// commit the completed lookup prefix before a later `__getitem__` error, while
/// an error from the keys iterator still leaves the receiver unchanged.
///
/// This covers `collections.ChainMap`, `UserDict`/`OrderedDict` subclasses, and
/// any user class that follows the duck-typed mapping protocol — without
/// requiring a concrete builtin `dict` backing (issue #2190).
///
/// Returns `Ok(false)` when `value` does not expose the protocol, allowing the
/// caller to fall back to iterable-of-pairs handling.  A native-backed `dict`
/// subclass is snapshotted before callbacks run: the callback may mutate that
/// same backing (`d.update(d)`), and no `RefCell` borrow or live iterator may
/// cross the mutation.
pub(crate) fn visit_mapping_pairs_via_protocol(
    interp: &mut Interpreter,
    value: &Value,
    mut visit: impl FnMut(&mut Interpreter, PyKey, Value) -> Result<()>,
) -> Result<bool> {
    let inst = match value.kind() {
        ValueKind::PyInstance(inst) => Rc::clone(inst),
        _ => return Ok(false),
    };
    let class = Rc::clone(&inst.borrow().class);
    // `dict` subclasses (OrderedDict / defaultdict / Counter): their `keys`
    // is a *builtin* method that is not present in `class.attrs`, so the
    // user-method `lookup_class_attr("keys")` below returns `None`.  Mirror
    // the `dict()` constructor's subclass handling so `{**subclass}`
    // materialises the same pairs as `dict(subclass)` (issue #2190).
    let getitem = lookup_class_attr(&class, "__getitem__");
    let is_dict_subclass = primitive_class_by_name("dict")
        .is_some_and(|dict_class| class_is_subclass_of(&class, &dict_class));
    if is_dict_subclass {
        // Concrete builtin backing dict (e.g. OrderedDict) — snapshot before
        // calling `visit`.  In particular, `dict.update` may be updating this
        // exact backing through a subclass receiver.
        if let Some(backing) = instance_builtin_data(&inst)
            && let Some(map) = backing.as_dict()
        {
            let pairs: Vec<(PyKey, Value)> = map.clone().into_iter().collect();
            // The visitor may mutate the same dict (for example
            // `d.update(d)`). Release the guarded read before user code.
            drop(map);
            for (key, item) in pairs {
                visit(interp, key, item)?;
            }
            return Ok(true);
        }
        // No builtin backing (defaultdict / Counter): iterate the instance for
        // its keys and subscript via `__getitem__`, exactly as `dict()` does.
        let getitem = match getitem {
            Some(m) => m,
            None => return Ok(false),
        };
        // These logical dict subclasses use a non-native backing in PyRust.
        // Snapshot their keys just as the native dict-subclass path does before
        // allowing callbacks to mutate an aliased destination.
        let keys = interp.collect_iterable(value)?;
        for k in keys {
            let v = invoke_class_method(
                interp,
                getitem.clone(),
                Value::py_instance(Rc::clone(&inst)),
                &[ExpandedCallArg {
                    name: None,
                    value: k.clone(),
                }],
            )?;
            let key = interp.value_to_pykey(&k)?;
            visit(interp, key, v)?;
        }
        return Ok(true);
    }
    let keys_method = match lookup_class_attr(&class, "keys") {
        Some(m) => m,
        None => return Ok(false),
    };
    let getitem = match getitem {
        Some(m) => m,
        None => return Ok(false),
    };
    // Call `m.keys()` and materialise its result (CPython does not require it to
    // be a list — any iterable of keys is accepted).  CPython intentionally
    // exhausts this iterable before the first `__getitem__`: a keys-iterator
    // error leaves the receiver untouched, and a truly unbounded keys iterator
    // never reaches lookup.  Only the resolved pairs are streamed to `visit`.
    let keys_source = invoke_class_method(
        interp,
        keys_method,
        Value::py_instance(Rc::clone(&inst)),
        &[],
    )?;
    let keys = interp.collect_iterable(&keys_source)?;
    for k in keys {
        let v = invoke_class_method(
            interp,
            getitem.clone(),
            Value::py_instance(Rc::clone(&inst)),
            &[ExpandedCallArg {
                name: None,
                value: k.clone(),
            }],
        )?;
        let key = interp.value_to_pykey(&k)?;
        visit(interp, key, v)?;
    }
    Ok(true)
}

/// Materialising compatibility wrapper for consumers that require an owned
/// mapping snapshot (for example `**` expansion).  Streaming consumers should
/// call [`visit_mapping_pairs_via_protocol`] directly so each completed lookup
/// can be committed before a later source error.
pub(crate) fn mapping_pairs_via_protocol(
    interp: &mut Interpreter,
    value: &Value,
) -> Result<Option<Vec<(PyKey, Value)>>> {
    let mut pairs = Vec::new();
    let is_mapping = visit_mapping_pairs_via_protocol(interp, value, |_, key, item| {
        pairs.push((key, item));
        Ok(())
    })?;
    Ok(is_mapping.then_some(pairs))
}

/// `true` if `value` is a mapping for printf-style `%`-formatting (issue #2089).
///
/// CPython enters mapping mode for a `%(key)` format when the rhs is not a
/// `tuple` and not a `str` and passes `PyMapping_Check` (has `mp_subscript`).
/// A plain `dict` always qualifies; a `PyInstance` qualifies when it exposes
/// `__getitem__` and is not a `tuple` or `str` subclass (`list`/`bytes`/
/// `bytearray`/`range` subclasses and custom `__getitem__` classes do qualify,
/// matching CPython — the subscript itself then raises the type-appropriate
/// error for a non-mapping key).
pub(crate) fn is_percent_format_mapping(value: &Value) -> bool {
    match value.kind() {
        ValueKind::Dict(_) => true,
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__getitem__").is_some()
                && find_immutable_primitive_base(&class) != Some("tuple")
                && find_scalar_primitive_base(&class) != Some("str")
        }
        _ => false,
    }
}

fn extract_optional_string(value: Value, name: &str) -> Result<Option<String>> {
    match value.kind() {
        ValueKind::Str(text) => Ok(Some(text.to_string())),
        ValueKind::None => Ok(None),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{} must be None or a string, not {}",
                name,
                value_type_name_str(&value)
            ),
        )),
    }
}

pub(crate) fn reject_keyword_args_expanded(
    function_name: &str,
    args: &[ExpandedCallArg],
) -> Result<()> {
    if args.iter().any(|a| a.name.is_some()) {
        // CPython raises TypeError with "takes no keyword arguments" when a
        // builtin accepts no keyword arguments at all (not a specific-kwarg
        // rejection).  Match that wording for parity.
        return Err(PyError::named(
            "TypeError",
            format!("{function_name}() takes no keyword arguments"),
        ));
    }
    Ok(())
}

/// Bind a builtin constructor's call args (positional + keyword) into a
/// per-parameter slot vector in declared parameter order, matching CPython
/// 3.12's argument-binding error semantics.
///
/// `params` lists every parameter in positional order; `keyword_ok` is the
/// matching mask of whether each parameter is keyword-acceptable (a `false`
/// entry is positional-only — supplying it by name yields the CPython
/// `'<name>' is an invalid keyword argument for <fn>()` error).  `max_args`
/// is the constructor's maximum total arity used for the "takes at most N
/// arguments" overflow check, which CPython performs *before* validating
/// keyword names.
///
/// Returns one slot per declared parameter (`None` for an unfilled slot), so
/// each constructor can apply its own per-parameter defaults / arity logic.
pub(crate) type ConstructorSlots = smallvec::SmallVec<[Option<Value>; 4]>;

pub(crate) fn bind_constructor_kwargs(
    function_name: &str,
    args: &[ExpandedCallArg],
    params: &[&str],
    keyword_ok: &[bool],
    max_args: usize,
) -> Result<ConstructorSlots> {
    debug_assert_eq!(params.len(), keyword_ok.len());

    // CPython checks total arity before validating individual keyword names:
    // `complex(1, 2, foo=3)` reports "takes at most 2 arguments (3 given)",
    // not the invalid-keyword error.  When *every* arg is a keyword, CPython
    // words it "takes at most N keyword arguments" instead.
    if args.len() > max_args {
        let noun = if args.iter().all(|a| a.name.is_some()) {
            "keyword arguments"
        } else {
            "arguments"
        };
        return Err(PyError::named(
            "TypeError",
            format!(
                "{function_name}() takes at most {max_args} {noun} ({} given)",
                args.len()
            ),
        ));
    }

    // SmallVec: every constructor here has ≤4 params (str=3, int/float/complex/
    // round=2), so the common case binds args with no heap allocation (#alloc).
    let mut slots: ConstructorSlots = smallvec::smallvec![None; params.len()];

    // Assign positional args to leading slots in order.
    for (next_pos, a) in args.iter().filter(|a| a.name.is_none()).enumerate() {
        // `args.len() <= max_args` already guarantees we don't overrun.
        slots[next_pos] = Some(a.value.clone());
    }

    // Bind keyword args by name.
    for a in args.iter().filter(|a| a.name.is_some()) {
        let name = a.name.as_ref().unwrap();
        match params.iter().position(|p| p == name) {
            Some(idx) if keyword_ok[idx] => {
                if slots[idx].is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "argument for {function_name}() given by name ('{name}') and position ({})",
                            idx + 1
                        ),
                    ));
                }
                slots[idx] = Some(a.value.clone());
            }
            // Either an unknown name, or a positional-only parameter supplied
            // by keyword — both surface as the invalid-keyword error.
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{name}' is an invalid keyword argument for {function_name}()"),
                ));
            }
        }
    }

    Ok(slots)
}
