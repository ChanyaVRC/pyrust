// Typed direct-dispatch handles for per-site `CallMethod` caches (#3379).
//
// The fast-path domain owns the per-site cache state and its guards; this
// module owns the decision of *which* built-in method a site resolves to and
// how that resolved method is executed.  The fast path never sees a method
// name table, a `pyrust_builtins` spec, or a registry key: it stores the opaque
// `Copy` handles below and hands them back here on every hit.

/// Per-kind typed route for an exact built-in container method.
#[derive(Clone, Copy, Debug)]
enum DirectContainerRoute {
    List(pyrust_builtins::list::MethodSpec),
    Dict(pyrust_builtins::dict::MethodSpec),
    Set(pyrust_builtins::set::MethodSpec),
}

/// A non-dunder method on an exact built-in container receiver, resolved once
/// for a `CallMethod` site.
///
/// The handle is valid only for a receiver of the same [`BuiltinContainerKind`]
/// (checked by [`Self::receiver_matches`]); the method name is captured by
/// the typed spec, so the cache key is the call site itself.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DirectContainerMethod {
    kind: BuiltinContainerKind,
    route: DirectContainerRoute,
}

impl DirectContainerMethod {
    /// Resolve `method` on an exact built-in container `receiver`.
    ///
    /// Only `list` / `dict` / `set` publish typed method specs.  Returns
    /// `None` for other receivers (`tuple` / `str` / `bytes` route by name
    /// inside their own dispatchers, so a cached handle could not skip any
    /// work), for dunder names (protocol dispatch with type-dependent
    /// diagnostics), and for names with no typed spec (their fallback owns
    /// the attribute error).
    pub(crate) fn resolve(receiver: &Value, method: &str) -> Option<Self> {
        let kind = BuiltinContainerKind::classify(Some(receiver))?;
        if method.starts_with("__") {
            return None;
        }
        let route = match kind {
            BuiltinContainerKind::List => {
                DirectContainerRoute::List(pyrust_builtins::list::method_spec(method)?)
            }
            BuiltinContainerKind::Dict => {
                DirectContainerRoute::Dict(pyrust_builtins::dict::method_spec(method)?)
            }
            BuiltinContainerKind::Set => {
                DirectContainerRoute::Set(pyrust_builtins::set::method_spec(method)?)
            }
            BuiltinContainerKind::Tuple
            | BuiltinContainerKind::Str
            | BuiltinContainerKind::Bytes => return None,
        };
        Some(Self { kind, route })
    }

    /// Whether `receiver` is an exact container of the kind this handle was
    /// resolved for.  Subclass instances are `PyInstance`s and never match.
    #[inline(always)]
    pub(crate) fn receiver_matches(self, receiver: Option<&Value>) -> bool {
        BuiltinContainerKind::classify(receiver) == Some(self.kind)
    }
}

/// A native-class instance method (`collections.deque.append`,
/// `Counter.most_common`, …) resolved once for a `CallMethod` site.
///
/// Holds the registry dispatch entry that [`invoke_class_method`] would reach
/// for the resolved unbound `BuiltinFunction`: a method descriptor with no
/// canonical primitive owner and no protocol/legacy special routing.  For
/// those, `invoke_class_method` performs no receiver validation of its own and
/// calls `dispatch([receiver, *args])`, so a cache hit calling the same entry
/// directly is behaviour-identical — it only skips re-resolving the registry
/// key and allocating a receiver `Value`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DirectNativeMethod {
    dispatch: crate::builtin_registry::BuiltinDispatchFn,
}

impl DirectNativeMethod {
    /// Resolve the unbound class attribute a `CallMethod` inline cache
    /// approved for a user/native-class instance.
    pub(crate) fn resolve(unbound: &Value) -> Option<Self> {
        let ValueKind::BuiltinFunction(name) = unbound.kind() else {
            return None;
        };
        let name: &'static str = name;
        let registration = crate::builtin_registry::lookup_registration(name)?;
        let metadata = registration.metadata;
        if metadata.kind != crate::builtin_registry::BuiltinCallableKind::MethodDescriptor
            || metadata.descriptor_owner_tag().is_some()
            || metadata.python_name().starts_with("__")
            || legacy_builtin_protocol_method(name).is_some()
            || is_interpreter_routed_builtin_method(name)
        {
            return None;
        }
        Some(Self {
            dispatch: registration.dispatch,
        })
    }
}

/// Built-in method keys that `invoke_class_method` routes to an interpreter
/// adapter instead of their registry entry.  They must never be cached as a
/// direct registry dispatch.
fn is_interpreter_routed_builtin_method(name: &str) -> bool {
    matches!(
        name,
        "BaseExceptionGroup.derive"
            | "BaseExceptionGroup.subgroup"
            | "BaseExceptionGroup.split"
            | "module.__init__"
            | "module.__repr__"
    )
}

impl Interpreter {
    /// Execute a cached exact-container method with positional `pos`.
    ///
    /// Equivalent to `dispatch_builtin_container_method(kind, receiver,
    /// method, pos, {}, false)` for the non-dunder `method` the handle was
    /// resolved from, minus the name match and generic dunder routing.
    pub(crate) fn call_direct_container_method(
        &mut self,
        handle: DirectContainerMethod,
        receiver: Value,
        pos: smallvec::SmallVec<[Value; 4]>,
    ) -> Result<Value> {
        let kw = PyDict::default();
        match handle.route {
            DirectContainerRoute::List(spec) => {
                self.dispatch_list_container_spec(receiver, spec, pos.into_vec(), &kw)
            }
            DirectContainerRoute::Dict(spec) => {
                self.dispatch_dict_container_spec(receiver, spec, pos, &kw, false)
            }
            DirectContainerRoute::Set(spec) => {
                self.dispatch_set_container_spec(receiver, spec, pos.into_vec(), &kw)
            }
        }
    }

    /// Allocation-free single-argument form of
    /// [`Self::call_direct_container_method`] for the hottest mutators
    /// (`list.append(x)`, `set.add(int | str)`).  Returns `Err(arg)` when the
    /// handle has no allocation-free body for this argument, handing the
    /// argument back for the general path.
    #[inline]
    pub(crate) fn try_call_direct_container_method_1arg(
        handle: DirectContainerMethod,
        receiver: &Value,
        arg: Value,
    ) -> Result<std::result::Result<Value, Value>> {
        match handle.route {
            DirectContainerRoute::List(spec)
                if matches!(spec.method(), pyrust_builtins::list::Method::Append) =>
            {
                receiver.list_push(arg)?;
                Ok(Ok(Value::none()))
            }
            DirectContainerRoute::Set(spec)
                if matches!(spec.method(), pyrust_builtins::set::Method::Add) =>
            {
                if let Some(i) = arg.as_int() {
                    receiver.set_add(PyKey::Int(i))?;
                    return Ok(Ok(Value::none()));
                }
                if arg.is_str() {
                    receiver.set_add(PyKey::Str(arg))?;
                    return Ok(Ok(Value::none()));
                }
                Ok(Err(arg))
            }
            _ => Ok(Err(arg)),
        }
    }

    /// Execute a cached native-class instance method: `receiver` followed by
    /// the positional `args`, exactly as `invoke_class_method` would pass them
    /// to the registry entry.
    pub(crate) fn call_direct_native_method(
        &mut self,
        handle: DirectNativeMethod,
        receiver: Value,
        args: smallvec::SmallVec<[Value; 4]>,
    ) -> Result<Value> {
        let mut combined = std::mem::take(&mut self.cold.invoke_arg_buf);
        combined.clear();
        combined.push(ExpandedCallArg {
            name: None,
            value: receiver,
        });
        combined.extend(
            args.into_iter()
                .map(|value| ExpandedCallArg { name: None, value }),
        );
        let result = (handle.dispatch)(self, &combined);
        combined.clear();
        self.cold.invoke_arg_buf = combined;
        result
    }
}
