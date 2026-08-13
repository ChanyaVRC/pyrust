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

/// Apply `staticmethod` / `classmethod` descriptor binding to a special-method
/// slot whose `UserFunction::kind` is not `Regular`, and invoke it.
///
/// `None` means "not a descriptor wrapper after all" (a `Builtin`-kind
/// function, or a classmethod whose receiver has no resolvable class); the
/// caller then falls through to its ordinary receiver-prepending path.
/// Deliberately `#[cold]` + `#[inline(never)]`: a staticmethod-in-a-dunder-slot
/// is rare, but [`invoke_class_method`] is the shared implicit-dunder dispatch
/// path that every `__init__`, `__getitem__`, `__len__` and `__eq__` call goes
/// through.  Inlining these arms into that hot function regressed ordinary
/// class construction by ~10% purely through lost inlining, so the cold work is
/// kept out-of-line (issue #2939).
#[cold]
#[inline(never)]
fn bind_descriptor_slot(
    interp: &mut Interpreter,
    f: &Rc<pyrust_core::UserFunction>,
    instance: &Value,
    args: &[ExpandedCallArg],
) -> Option<Result<Value>> {
    match f.kind {
        // `staticmethod.__get__` yields the wrapped callable untouched — no
        // receiver is passed.
        pyrust_core::UserFunctionKind::StaticMethod => {
            let unwrapped = match f.wrapped_func.as_ref() {
                Some(inner) => Value::user_function(Rc::clone(inner)),
                None => {
                    Value::with_function_kind(Rc::clone(f), pyrust_core::UserFunctionKind::Regular)
                }
            };
            Some(interp.call_function_expanded(unwrapped, args))
        }
        // `classmethod.__get__` binds the owning class in place of the
        // receiver.  Resolve it exactly as the wrapped-classmethod branch in
        // `invoke_class_method` does, so both spellings of `@classmethod` agree
        // on what `cls` is.
        pyrust_core::UserFunctionKind::ClassMethod => {
            let owner = match instance.kind() {
                ValueKind::PyClass(class) => Some(Rc::clone(class)),
                ValueKind::PyInstance(object) => Some(Rc::clone(&object.borrow().class)),
                _ => match value_class(instance).kind() {
                    ValueKind::PyClass(class) => Some(Rc::clone(class)),
                    _ => None,
                },
            }?;
            let bound = Value::class_bound_method(Rc::clone(f), owner);
            Some(interp.call_function_expanded(bound, args))
        }
        _ => None,
    }
}

/// Bind a non-function `staticmethod` / `classmethod` wrapper for class-level
/// descriptor access. This is the non-invoking form of the typed wrapper logic
/// in [`invoke_class_method`], used when a protocol must add its own explicit
/// arguments after binding (notably `type.__call__` dispatching `__new__`).
/// `None` means `value` is not one of these wrappers.
#[inline]
pub(crate) fn bind_class_level_method_wrapper(
    value: &Value,
    class: &Rc<RefCell<PyClass>>,
) -> Result<Option<Value>> {
    if !matches!(value.kind(), ValueKind::BuiltinObject { .. }) {
        return Ok(None);
    }
    if let Some(binding) = pyrust_builtins::classmethod::as_class_method_any(value) {
        return pyrust_builtins::classmethod::bind_wrapped_class_method(binding, Rc::clone(class))
            .map(Some);
    }
    Ok(pyrust_builtins::classmethod::as_static_method_any(value))
}

/// Invoke a `__get__` slot the way CPython's `slot_tp_descr_get` does.
///
/// `__get__` is the one special method CPython deliberately does *not* run
/// through the descriptor protocol: `slot_tp_descr_get` resolves it with a raw
/// `_PyType_Lookup` and calls whatever it finds directly, as
/// `get(self, obj, objtype)`.  Consequences, all verified against CPython 3.12:
///
/// - a `staticmethod` `__get__` still receives the descriptor itself as its
///   first argument (a `staticmethod` object is callable since 3.10 and just
///   forwards), so `__get__(a, b, c)` works and `__get__(obj, objtype=None)`
///   raises `TypeError: … takes from 1 to 2 positional arguments but 3 were
///   given`;
/// - a `classmethod` `__get__` is a plain `TypeError` — a `classmethod` object
///   is not callable at all.
///
/// This is deliberately asymmetric with `__set__` / `__delete__`, which go
/// through `vectorcall_method` and therefore *do* bind — those keep using
/// [`invoke_class_method`].  Routing `__get__` through the descriptor binding
/// too would silently invoke a `staticmethod`/`classmethod` getter that CPython
/// rejects (issue #2939 review).
///
/// `None` means the slot carries no descriptor semantics of its own, so the
/// caller falls through to its ordinary [`invoke_class_method`] path.
///
/// Deliberately `#[cold]` + `#[inline(never)]`, and deliberately *not* wrapped
/// in a helper that itself calls [`invoke_class_method`]: handing that function
/// an extra call site perturbs its inlining and cost ~6.6% on ordinary class
/// construction — a path that never executes any of this code.  The caller
/// therefore keeps its single, unchanged `invoke_class_method` tail call and
/// merely guards it with a `kind != Regular` compare (issue #2939 review).
#[cold]
#[inline(never)]
pub(crate) fn descriptor_get_slot_raw_call(
    interp: &mut Interpreter,
    f: &Rc<pyrust_core::UserFunction>,
    instance: &Value,
    args: &[ExpandedCallArg],
) -> Option<Result<Value>> {
    match f.kind {
        pyrust_core::UserFunctionKind::ClassMethod => Some(Err(pyrust_core::py_err!(
            "TypeError",
            "'classmethod' object is not callable"
        ))),
        // Raw call: the descriptor is passed positionally, exactly as
        // `PyObject_CallFunctionObjArgs(get, self, obj, type)` does.
        pyrust_core::UserFunctionKind::StaticMethod => {
            let func = Rc::clone(f);
            Some(interp.call_user_function_expanded(func, args, std::slice::from_ref(instance)))
        }
        _ => None,
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
            // The binding itself lives in the `#[cold]` `bind_descriptor_slot`
            // so this arm keeps the exact shape (and inlining budget) it had
            // before: a `Regular` slot — the overwhelmingly common case, and
            // the one every `__init__` / `__getitem__` / `__len__` dispatch
            // takes — pays one discriminant compare on a `Copy` tag already
            // resident in the `UserFunction` just matched, then falls into the
            // unchanged receiver-prepending tail.  Inlining the wrapper arms
            // here instead cost ~10% on class construction even though the
            // added code never ran on that path.
            if f.kind != pyrust_core::UserFunctionKind::Regular
                && let Some(result) = bind_descriptor_slot(interp, f, &instance, args)
            {
                return result;
            }
            let func = Rc::clone(f);
            return interp.call_user_function_expanded(func, args, &[instance]);
        }
        ValueKind::BuiltinFunction(name) => {
            let resolution = resolve_and_validate_builtin_method(&method_val, &instance)?;
            // Issue #2948: a flat builtin function (`len`, `repr`, `iter`, `id`) is a
            // `builtin_function_or_method`, which implements NO descriptor protocol.
            // CPython therefore calls it with `args` alone: `class C: __len__ = len`
            // makes `len(C())` evaluate `len()` and raise `TypeError: len() takes
            // exactly one argument (0 given)`, and `C.__len__ is len` /
            // `C().__len__ is len` are both True.
            //
            // Prepending the receiver instead made `len(C())` evaluate `len(self)`,
            // which re-entered the same slot and overflowed the *native* stack —
            // aborting the process (SIGABRT) with no catchable Python exception.
            // `__hash__ = id` does not re-enter and so silently returned `id(self)`.
            //
            // The explicit attribute path already treats these as non-descriptors
            // (see `bind_builtin_attribute` / `instance_attributes.rs`), so calling
            // the slot unbound here simply makes implicit dunder dispatch agree with
            // `getattr(C(), "__len__")`.  The arity error then falls out of the
            // builtin's own signature check, which reproduces CPython's wording for
            // free.
            //
            // Two guards keep this off every path that legitimately binds a receiver:
            //
            // * `canonical_owner.is_none()` — every primitive slot wrapper
            //   (`list.__len__`, `dict.__contains__`, `bytes.__iter__`, the
            //   `bytearray` ops-table methods) is claimed by a canonical owner and
            //   short-circuits before the metadata probe.
            // * the `builtins` declaring module — the registry's `ModuleFunction` tag
            //   alone is NOT sufficient.  `pyrust-derive` infers the tag from a dot
            //   in the declared `py_name`, and some interpreter-owned class-slot
            //   sentinels deliberately use an undotted name to steer a separate
            //   name-based check elsewhere: `typing._generic_cgi` / `_union_cgi` /
            //   `_optional_cgi` are installed as `__class_getitem__` slots and read
            //   the receiver class to pick the alias origin (see the comment above
            //   `_generic_cgi` in `builtin_modules/bodies/typing.rs`), yet they
            //   register as `ModuleFunction`.  Unbinding them silently resolved
            //   `Union[...].__origin__` against the current `typing` generation
            //   instead of the receiver's.  Restricting the rule to the flat
            //   `builtins` namespace covers exactly the user-nameable bare builtins
            //   this issue is about and cannot reach those sentinels.  Non-`builtins`
            //   module functions in dunder slots (`__len__ = math.sqrt`) therefore
            //   still bind and remain divergent — tracked as a follow-up, because
            //   fixing them means re-modelling those sentinels rather than widening
            //   this gate.
            if resolution.canonical_owner.is_none() {
                // Reuse the registration `resolve_and_validate_builtin_method`
                // already binary-searched.  `builtin_callable_metadata` repeats
                // that search, and EVERY builtin dunder whose owner is not a
                // canonical primitive reaches this branch — `Counter.__getitem__`,
                // `deque.__len__`, `OrderedDict.__setitem__`, the `io` and
                // `Decimal` dunders — so the duplicate `&str` binary search cost
                // ~17% on a Counter/deque/OrderedDict dunder loop.  The fallback
                // is only needed for a name with no registration at all, which is
                // the cold path that errors or re-binds below.
                let metadata = match resolution.registration {
                    Some(entry) => entry.metadata,
                    None => builtin_methods::builtin_callable_metadata(name),
                };
                if metadata.kind == crate::builtin_registry::BuiltinCallableKind::ModuleFunction
                    && metadata.python_module() == Some("builtins")
                {
                    // `method_val` is still borrowed by the enclosing `match`, so
                    // hand the callable over by clone (an Rc bump on an already-cold
                    // path) rather than restructuring the hot arm's control flow.
                    return interp.call_function_expanded(method_val.clone(), args);
                }
            }
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
                let rest: Vec<Value> = args
                    .iter()
                    .filter(|a| a.name.is_none())
                    .map(|a| a.value.clone())
                    .collect();
                // Resolve a builtin subclass to its backing for the protocol
                // body, but keep the Python-visible receiver available for the
                // return-self contract of mutable in-place wrappers (#2990).
                if let ValueKind::PyInstance(inst) = instance.kind()
                    && let Some(backing) = instance_builtin_data(inst)
                {
                    if builtin_methods::is_mutable_builtin_inplace_dunder(&method) {
                        return interp.dispatch_builtin_subclass_protocol_dunder(
                            &method, &instance, backing, rest,
                        );
                    }
                    return interp.dispatch_builtin_protocol_dunder(&method, backing, rest);
                }
                return interp.dispatch_builtin_protocol_dunder(&method, instance.clone(), rest);
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
    // Issue #2944: the resolved slot is not a plain function.  Before deciding
    // whether it is callable, honour the descriptor protocol — CPython's slot
    // lookup binds *any* descriptor, not just the function / staticmethod /
    // classmethod shapes handled above, so a `property` or a user object with
    // `__get__` runs its getter here and the getter's *result* is what gets
    // called.  That ordering is also what makes the not-callable error name the
    // bound value: `__len__ = property(lambda self: 4)` reports `'int' object is
    // not callable`, not `'property'`.
    if let Some(result) = bind_slot_descriptor(interp, &method_val, &instance, args) {
        return result;
    }
    call_slot_value_unbound(interp, method_val, args)
}

/// Bind a descriptor sitting in a special-method slot the way CPython's
/// `lookup_maybe_method` does — `__get__(instance, type(instance))` — and call
/// the result with the slot's own arguments.
///
/// `None` means the slot is not a descriptor and the caller should dispatch it
/// directly.
///
/// The `__get__` result is dispatched *unbound*: CPython calls it once and
/// never re-enters `tp_descr_get`, so a descriptor that returns another
/// descriptor yields "not callable" rather than binding a second time.
///
/// `#[cold]` + `#[inline(never)]` for the same reason as
/// [`bind_descriptor_slot`]: [`invoke_class_method`] is the shared implicit
/// dunder path, and a descriptor in a dunder slot is rare enough that this must
/// not cost the common `UserFunction` / `BuiltinFunction` arms any inlining
/// budget (issue #2939).
#[cold]
#[inline(never)]
fn bind_slot_descriptor(
    interp: &mut Interpreter,
    method_val: &Value,
    instance: &Value,
    args: &[ExpandedCallArg],
) -> Option<Result<Value>> {
    if !slot_is_descriptor(method_val) {
        return None;
    }
    // CPython passes `type(instance)` as the descriptor's `objtype`; for a slot
    // resolved on a metaclass the receiver *is* the class, so `value_class`
    // yields the metaclass exactly as `Py_TYPE(self)` would.
    let owner = value_class(instance);
    let bound = match call_descriptor_get(interp, method_val, instance.clone(), owner, "") {
        Ok(bound) => bound,
        Err(e) => return Some(Err(e)),
    };
    Some(call_slot_value_unbound(interp, bound, args))
}

/// Call a resolved special-method slot value directly, with no descriptor
/// binding and no receiver prepended, or raise CPython's "not callable" keyed
/// on that value's type.
///
/// Issue #2054: such a slot may still be callable — a bound method, a class
/// object, or a callable *instance* (an object whose class defines `__call__`).
/// CPython invokes whatever the slot resolves to.  Having already been found
/// *not* to be a descriptor, it does NOT receive the receiver as `self`:
/// `__len__ = Caller()` calls `Caller()()` with no implicit self, and
/// `__add__ = Caller()` calls `Caller()(other)`.
///
/// Genuinely non-callable slots (`Foo.__len__ = 5`) raise the standard "object
/// is not callable" keyed on the *resolved value's* type, not the owning class:
/// `len(D())` with `__len__ = 5` -> "'int' object is not callable".  Match that
/// exactly so every implicit-dunder dispatch path agrees with CPython 3.12
/// (issue #1963 / #2055).
///
/// Split out of [`invoke_class_method`] so the descriptor-binding path can
/// dispatch a `__get__` result through the identical rules, and so
/// `call_descriptor_get`'s raw `__get__` invocation can reuse them.
pub(crate) fn call_slot_value_unbound(
    interp: &mut Interpreter,
    method_val: Value,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    if slot_is_callable(&method_val) {
        interp.call_function_expanded(method_val, args)
    } else {
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object is not callable",
                pyrust_core::error_type_name(&method_val)
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
        let v = if let Some(getitem) = &getitem {
            invoke_class_method(
                interp,
                getitem.clone(),
                Value::py_instance(Rc::clone(&inst)),
                &[ExpandedCallArg {
                    name: None,
                    value: k.clone(),
                }],
            )?
        } else {
            // `keys()` establishes the mapping protocol. CPython does not
            // require `__getitem__` until a yielded key is actually consumed,
            // so an empty keys iterable succeeds and a nonempty one reaches
            // the ordinary subscription TypeError.
            interp.eval_index(value, k.clone())?
        };
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
