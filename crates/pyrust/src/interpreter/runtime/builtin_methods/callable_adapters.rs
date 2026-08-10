// Adapter boundary for callable objects supplied by the builtin layer.
//
// Generic call dispatch asks this service whether a Value is one of the
// builtin callable representations. Exact builtin names, descriptor policies,
// and special constructors stay here rather than leaking into the call router.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolvedMethodCallShape {
    Fused,
    Expanded,
}

impl ResolvedMethodCallShape {
    /// Mirror CPython 3.12's `maybe_optimize_method_call` stack-use cutoff.
    /// Keyword calls reserve one additional slot for their names tuple.
    fn preserves_direct_descriptor_owner(self, args: &[ExpandedCallArg]) -> bool {
        const CPYTHON_METHOD_CALL_STACK_GUIDELINE: usize = 30;

        if self != Self::Fused {
            return false;
        }
        if args.len() + 1 < CPYTHON_METHOD_CALL_STACK_GUIDELINE {
            return true;
        }
        if args.len() >= CPYTHON_METHOD_CALL_STACK_GUIDELINE {
            return false;
        }
        // Exactly 29 arguments remain: positional-only calls stay fused, but
        // any keyword reserves the 30th stack slot and forces captured-call
        // semantics. Other arities never need to scan the argument names.
        args.iter().all(|arg| arg.name.is_none())
    }
}

/// Whether a `BuiltinObject` is one of the typed adapters actually handled by
/// [`Interpreter::try_call_builtin_callable`].
///
/// Python's `callable()` and argument validators consume this same inventory
/// instead of maintaining parallel category lists that can drift when a new
/// descriptor adapter is added.
pub(crate) fn is_builtin_callable_adapter(value: &Value) -> bool {
    pyrust_builtins::native_builtin_callable::as_native_static_builtin(value).is_some()
        || pyrust_builtins::classmethod::as_class_bound_any(value).is_some()
        || pyrust_builtins::super_bound_builtin::as_super_bound_builtin(value).is_some()
        || pyrust_builtins::type_call_wrapper::as_type_call_wrapper(value).is_some()
        || pyrust_builtins::unbound_method_descriptor::as_unbound_method_descriptor(value).is_some()
        || pyrust_builtins::bound_method::is_bound_method(value)
        || pyrust_builtins::property::as_property_method(value).is_some()
        || pyrust_builtins::property::property_partial_slot(value)
            .is_some_and(|slot| slot.is_some())
        || pyrust_builtins::classmethod::as_class_method_get_binder(value).is_some()
        || pyrust_builtins::classmethod::as_static_method_get_binder(value).is_some()
        || pyrust_builtins::numeric_attrs_descriptor::as_method_descriptor(value).is_some()
        || pyrust_builtins::generic_alias::as_generic_alias_origin(value).is_some()
}

/// Whether `value` is the callable adapter produced by binding an arbitrary
/// Python-created `classmethod` payload.
///
/// Metaclass hook gates use this typed question without decoding a concrete
/// builtin representation in the generic call runtime (#2947).
#[inline]
pub(crate) fn is_class_bound_any_callable(value: &Value) -> bool {
    pyrust_builtins::classmethod::as_class_bound_any(value).is_some()
}

/// Decode a legacy, unregistered protocol sentinel at the concrete built-in
/// adapter boundary.
///
/// New registered descriptors carry typed owner/name metadata. Generator and
/// a few historical protocol sentinels still use a qualified dispatch key;
/// generic call routing receives only the resulting method decision.
pub(crate) fn legacy_builtin_protocol_method(registry_key: &'static str) -> Option<&'static str> {
    let (type_name, method) = registry_key.split_once('.')?;
    (method.starts_with("__") && is_protocol_dunder(type_name, method)).then_some(method)
}

/// Adapt an unregistered built-in method inherited by a primitive subclass to
/// its opaque backing value.
///
/// Qualified-key decoding is intentionally confined here. The generic
/// `invoke_class_method` router consumes the ready-to-call bound value.
pub(crate) fn bind_legacy_builtin_subclass_backing(
    registry_key: &'static str,
    instance: &Value,
) -> Option<Value> {
    let ValueKind::PyInstance(instance) = instance.kind() else {
        return None;
    };
    let (type_name, method) = registry_key.split_once('.')?;
    let backing = instance_builtin_data(instance)?;
    (pyrust_core::builtin_type_name(&backing) == type_name)
        .then(|| pyrust_builtins::bound_method::bound_method(method, backing))
}

impl Interpreter {
    /// Call a value resolved by a `CallMethod` opcode without exposing its
    /// concrete builtin representation to the fast-path domain.
    ///
    /// A compiler-fused positional/keyword call within CPython's stack-use
    /// cutoff is a direct descriptor invocation only when the resolved bound
    /// callable still targets the object on which the lookup occurred. Larger
    /// calls, expanded calls, and callables returned from an unrelated
    /// attribute/property keep normal captured-bound-method semantics.
    pub(super) fn call_resolved_method(
        &mut self,
        lookup_receiver: &Value,
        resolved: Value,
        args: &[ExpandedCallArg],
        shape: ResolvedMethodCallShape,
    ) -> Result<Value> {
        if shape.preserves_direct_descriptor_owner(args)
            && let Some((name, receiver)) =
                pyrust_builtins::bound_method::as_bound_method(&resolved)
            && receiver.is_identical_to(lookup_receiver)
        {
            return self.call_bound_method_dispatch(name, receiver, args);
        }
        self.call_function_expanded(resolved, args)
    }

    /// Preserve object identity for the VM's borrowed-register `id(x)` fast
    /// path.
    ///
    /// This used to answer only for values with an *allocation* identity and
    /// hand everything else to the registered builtin, which is how the two
    /// paths came to disagree (#2956).  Both now read the one definition,
    /// [`Value::object_id`], so the fast path is a pure short-cut rather than
    /// a second implementation.
    pub(super) fn try_identity_builtin_call(function: &Value, argument: &Value) -> Option<Value> {
        matches!(function.kind(), ValueKind::BuiltinFunction("id")).then(|| argument.object_id())
    }

    /// Handle the concrete class objects supplied by the builtin layer.
    ///
    /// Generic call routing owns the `PyClass` classification and ordinary
    /// class construction; exact builtin identities and constructors remain
    /// confined to this adapter.
    #[inline]
    pub(super) fn try_call_builtin_class(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Option<Value>> {
        if let Some(dispatch) = primitive_class_dispatch(class) {
            return dispatch(self, args).map(Some);
        }
        if let Some(value) = self.try_call_typing_marker(class, args)? {
            return Ok(Some(value));
        }
        match class.borrow().canonical_tag {
            Some(pyrust_core::CanonicalClassTag::MappingProxy) => {
                return construct_mapping_proxy(args).map(Some);
            }
            Some(pyrust_core::CanonicalClassTag::GenericAlias) => {
                if args.iter().any(|arg| arg.name.is_some()) {
                    return Err(pyrust_core::type_err!(
                        "GenericAlias() takes no keyword arguments"
                    ));
                }
                if args.len() != 2 {
                    return Err(pyrust_core::type_err!(
                        "GenericAlias expected 2 arguments, got {}",
                        args.len()
                    ));
                }
                let origin = args[0].value.clone();
                let index = args[1].value.clone();
                let type_args = if matches!(index.kind(), ValueKind::Tuple(_)) {
                    index
                } else {
                    Value::tuple(vec![index])
                };
                return Ok(Some(pyrust_builtins::generic_alias::generic_alias(
                    origin, type_args,
                )));
            }
            _ => {}
        }
        Ok(None)
    }

    /// Invoke a native classmethod recovered from a validated attribute-cache
    /// plan.
    ///
    /// The descriptor provider owns target identity and receiver construction;
    /// this callable adapter owns prepending that receiver and dispatching the
    /// wrapped registry builtin.  The fast-path domain therefore never
    /// interprets a Python method name or a builtin registry key.
    pub(super) fn try_call_cached_native_class_method(
        &mut self,
        plan: &pyrust_builtins::classmethod::NativeClassMethodCachePlan,
        class: &Rc<RefCell<PyClass>>,
        args: &[Value],
    ) -> Option<Result<Value>> {
        let (wrapped, receiver) =
            pyrust_builtins::classmethod::cached_native_class_method_call(plan, class)?;
        let mut combined = std::mem::take(&mut self.invoke_arg_buf);
        combined.clear();
        combined.push(ExpandedCallArg {
            name: None,
            value: receiver,
        });
        combined.extend(
            args.iter()
                .cloned()
                .map(|value| ExpandedCallArg { name: None, value }),
        );
        let result = self.call_builtin_function_value(&wrapped, &combined);
        self.invoke_arg_buf = combined;
        Some(result)
    }

    pub(super) fn try_call_builtin_callable(
        &mut self,
        function: &Value,
        args: &[ExpandedCallArg],
    ) -> Result<Option<Value>> {
        if let Some(call) =
            pyrust_builtins::native_builtin_callable::as_native_static_builtin(function)
        {
            if let Some(intrinsic) = call.intrinsic {
                let result = match intrinsic {
                    pyrust_builtins::native_builtin_callable::NativeBuiltinIntrinsic::IndexProtocol => {
                        if args.iter().any(|arg| arg.name.is_some()) {
                            return Err(pyrust_core::type_err!(
                                "_operator.index() takes no keyword arguments"
                            ));
                        }
                        if args.len() != 1 {
                            return Err(pyrust_core::type_err!(
                                "_operator.index() takes exactly one argument ({} given)",
                                args.len()
                            ));
                        }
                        let result = self.value_to_index(&args[0].value, |value| {
                            pyrust_core::type_err!(
                                "'{}' object cannot be interpreted as an integer",
                                pyrust_core::builtin_type_name(value)
                            )
                        })?;
                        if let ValueKind::Bool(value) = result.kind() {
                            Value::int(value as i64)
                        } else {
                            result
                        }
                    }
                };
                return Ok(Some(result));
            }
            if let Some(receiver) = call.receiver {
                // The descriptor provider guarantees that native class-bound
                // wrappers contain a BuiltinFunction. Dispatch that payload at
                // this adapter boundary instead of recursively entering the
                // generic callable classifier and probing every adapter again.
                let mut combined = std::mem::take(&mut self.invoke_arg_buf);
                combined.clear();
                combined.push(ExpandedCallArg {
                    name: None,
                    value: receiver,
                });
                combined.extend(args.iter().cloned());
                let result = if matches!(call.wrapped.kind(), ValueKind::BuiltinFunction(_)) {
                    self.call_builtin_function_value(&call.wrapped, &combined)
                } else {
                    self.call_function_expanded(call.wrapped, &combined)
                };
                self.invoke_arg_buf = combined;
                return result.map(Some);
            }
            if matches!(call.wrapped.kind(), ValueKind::BuiltinFunction(_)) {
                return self
                    .call_builtin_function_value(&call.wrapped, args)
                    .map(Some);
            }
            return self.call_function_expanded(call.wrapped, args).map(Some);
        }

        if let Some((wrapped, class)) = pyrust_builtins::classmethod::as_class_bound_any(function) {
            let mut combined = ExpandedArgBuf::with_capacity(args.len() + 1);
            combined.push(ExpandedCallArg {
                name: None,
                value: Value::py_class(class),
            });
            combined.extend(args.iter().cloned());
            return self.call_function_expanded(wrapped, &combined).map(Some);
        }

        if let Some((fn_name, instance)) =
            pyrust_builtins::super_bound_builtin::as_super_bound_builtin(function)
        {
            if let Some(dispatch) = crate::builtin_registry::lookup(&fn_name) {
                let mut combined = ExpandedArgBuf::with_capacity(args.len() + 1);
                combined.push(ExpandedCallArg {
                    name: None,
                    value: instance,
                });
                combined.extend(args.iter().cloned());
                return dispatch(self, &combined).map(Some);
            }
            if let Some(method_name) = fn_name.split_once('.').map(|(_, method)| method) {
                if is_mutable_builtin_inplace_dunder(method_name)
                    && let Some(backing) = builtin_data_backing(&instance)
                {
                    let bound =
                        pyrust_builtins::bound_method::bound_method(method_name, backing.clone());
                    let result = self.call_function_expanded(bound, args)?;
                    return Ok(Some(restore_builtin_subclass_inplace_result(
                        method_name,
                        &instance,
                        &backing,
                        result,
                    )));
                }
                let receiver = builtin_data_backing(&instance).unwrap_or(instance);
                let bound = pyrust_builtins::bound_method::bound_method(method_name, receiver);
                return self.call_function_expanded(bound, args).map(Some);
            }
        }

        if let Some(class) = pyrust_builtins::type_call_wrapper::as_type_call_wrapper(function) {
            return self.call_function_expanded(class, args).map(Some);
        }

        if matches!(function.kind(), ValueKind::BuiltinFunction(_)) {
            return self.call_builtin_function_value(function, args).map(Some);
        }

        if let Some((owner_value, method, callable)) =
            pyrust_builtins::unbound_method_descriptor::as_unbound_method_descriptor(function)
        {
            let ValueKind::PyClass(owner) = owner_value.kind() else {
                return self.call_function_expanded(callable, args).map(Some);
            };
            let owner = Rc::clone(owner);
            let owner_display = class_descriptor_display_name(&owner);
            let owner_bare = owner.borrow().qualname.clone();
            let receiver = args
                .iter()
                .find(|arg| arg.name.is_none())
                .map(|arg| &arg.value)
                .ok_or_else(|| {
                    pyrust_core::type_err!(
                        "unbound method {owner_bare}.{method}() needs an argument"
                    )
                })?;
            let receiver_matches = match receiver.kind() {
                ValueKind::PyInstance(instance) => {
                    class_is_subclass_of(&Rc::clone(&instance.borrow().class), &owner)
                }
                _ => false,
            };
            if !receiver_matches {
                let actual = pyrust_core::builtin_type_name(receiver);
                return Err(pyrust_core::type_err!(
                    "descriptor '{method}' for '{owner_display}' objects doesn't apply to a '{actual}' object"
                ));
            }
            return self.call_function_expanded(callable, args).map(Some);
        }

        if let Some((name, receiver)) = pyrust_builtins::bound_method::as_bound_method(function) {
            return self
                .call_captured_bound_method_dispatch(name, receiver, args)
                .map(Some);
        }

        if let Some((property, kind)) = pyrust_builtins::property::as_property_method(function) {
            if args.iter().any(|arg| arg.name.is_some()) {
                return Err(pyrust_core::type_err!(
                    "this method takes no keyword arguments"
                ));
            }
            let positional = args.iter().map(|arg| arg.value.clone()).collect::<Vec<_>>();
            return dispatch_property_method(self, &property, kind, &positional).map(Some);
        }

        if pyrust_builtins::property::property_partial_slot(function)
            .is_some_and(|slot| slot.is_some())
        {
            let (fget, fset, fdel, slot) =
                pyrust_builtins::property::with_property(function, |state| {
                    (
                        Rc::clone(&state.fget),
                        Rc::clone(&state.fset),
                        Rc::clone(&state.fdel),
                        state.partial_slot.expect("guard ensured Some"),
                    )
                })
                .expect("guard ensured property");
            let accessor_name = match slot {
                0 => "property.getter",
                1 => "property.setter",
                _ => "property.deleter",
            };
            if args.iter().any(|arg| arg.name.is_some()) {
                return Err(pyrust_core::type_err!(
                    "{accessor_name}() takes no keyword arguments"
                ));
            }
            if args.len() != 1 {
                return Err(pyrust_core::type_err!(
                    "{accessor_name}() takes exactly one argument ({} given)",
                    args.len()
                ));
            }
            let replacement = args[0].value.clone();
            let (fget, fset, fdel) = match slot {
                0 => (replacement, (*fset).clone(), (*fdel).clone()),
                1 => ((*fget).clone(), replacement, (*fdel).clone()),
                2 => ((*fget).clone(), (*fset).clone(), replacement),
                _ => unreachable!(),
            };
            return Ok(Some(pyrust_builtins::property::property(fget, fset, fdel)));
        }

        if let Some(function) = pyrust_builtins::classmethod::as_class_method_get_binder(function) {
            let instance = args
                .first()
                .map(|arg| arg.value.clone())
                .unwrap_or_else(Value::none);
            let owner = args
                .get(1)
                .map(|arg| arg.value.clone())
                .unwrap_or_else(Value::none);
            if instance.is_none() && owner.is_none() {
                return Err(pyrust_core::type_err!("__get__(None, None) is invalid"));
            }
            let value = match owner.kind() {
                ValueKind::PyClass(class) => Value::class_bound_method(function, Rc::clone(class)),
                _ => Value::user_function(function),
            };
            return Ok(Some(value));
        }

        if let Some(function) = pyrust_builtins::classmethod::as_static_method_get_binder(function)
        {
            let instance = args
                .first()
                .map(|arg| arg.value.clone())
                .unwrap_or_else(Value::none);
            let owner = args
                .get(1)
                .map(|arg| arg.value.clone())
                .unwrap_or_else(Value::none);
            if instance.is_none() && owner.is_none() {
                return Err(pyrust_core::type_err!("__get__(None, None) is invalid"));
            }
            let value = if let Some(inner) = function.wrapped_func.as_ref() {
                Value::user_function(Rc::clone(inner))
            } else {
                Value::with_function_kind(function, pyrust_core::UserFunctionKind::Regular)
            };
            return Ok(Some(value));
        }

        if let Some((attribute, class_name)) =
            pyrust_builtins::numeric_attrs_descriptor::as_method_descriptor(function)
        {
            if args.iter().any(|arg| arg.name.is_some()) {
                return Err(pyrust_core::type_err!(
                    "{class_name}.{attribute}() takes no keyword arguments"
                ));
            }
            let Some(receiver) = args.first() else {
                return Err(pyrust_core::descriptor_needs_arg!(
                    attribute, class_name, method
                ));
            };
            let method = self.get_attr(&receiver.value, attribute)?;
            return self.call_function_expanded(method, &args[1..]).map(Some);
        }

        if let Some(origin) = pyrust_builtins::generic_alias::as_generic_alias_origin(function) {
            return self.call_function_expanded(origin, args).map(Some);
        }

        Ok(None)
    }

    /// Dispatch a value already proven to be `BuiltinFunction`.
    ///
    /// Native descriptor adapters call this directly so their wrapped
    /// function does not re-enter generic callable classification. Plain
    /// builtin calls use the same body, keeping registry/protocol/unregistered
    /// precedence identical.
    pub(super) fn call_builtin_function_value(
        &mut self,
        function: &Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        let ValueKind::BuiltinFunction(name) = function.kind() else {
            unreachable!("builtin function adapter requires BuiltinFunction");
        };
        if let Some((type_name, method)) = name.split_once('.')
            && method.starts_with("__")
            && is_protocol_dunder(type_name, method)
            && let Some(receiver_arg) = args.first()
        {
            let receiver = receiver_arg.value.clone();
            let receiver_matches = receiver_arg.name.is_none()
                && !matches!(receiver.kind(), ValueKind::PyInstance(_))
                // Dictionary views have a typed unbound-descriptor adapter
                // that preserves plain-vs-ordered ownership and inherited
                // method policy. Do not bypass it through the generic layout
                // table fast path.
                && pyrust_builtins::dict_views::view_kind(&receiver).is_none()
                && pyrust_core::builtin_layout_type_name(&receiver) == type_name;
            if receiver_matches {
                if args[1..].iter().any(|arg| arg.name.is_some()) {
                    return Err(if is_named_protocol_wrapper(method, type_name) {
                        pyrust_core::type_err!("{type_name}.{method}() takes no keyword arguments")
                    } else {
                        pyrust_core::type_err!("wrapper {method}() takes no keyword arguments")
                    });
                }
                let positional = args[1..]
                    .iter()
                    .filter(|arg| arg.name.is_none())
                    .map(|arg| arg.value.clone())
                    .collect();
                return self.dispatch_builtin_protocol_dunder(method, receiver, positional);
            }
        }

        if let Some(dispatch) = crate::builtin_registry::lookup(name) {
            return dispatch(self, args);
        }
        self.call_unregistered_builtin_function(function, args)
    }

    /// Dispatch an MRO-resolved built-in `__new__` sentinel.
    ///
    /// Class construction has already classified the callable and prepended
    /// `cls`, so it should not re-run generic descriptor/protocol adaptation.
    /// Keeping the registry lookup here preserves the builtin-method ownership
    /// boundary without adding work to the constructor hot path.
    #[inline]
    pub(super) fn call_builtin_new_value(
        &mut self,
        function: &Value,
        args: &[ExpandedCallArg],
    ) -> Result<Value> {
        let ValueKind::BuiltinFunction(name) = function.kind() else {
            unreachable!("built-in __new__ adapter requires BuiltinFunction");
        };
        let dispatch = crate::builtin_registry::lookup(name).ok_or_else(|| {
            PyError::Runtime(format!(
                "internal: __new__ builtin '{name}' not in registry"
            ))
        })?;
        dispatch(self, args)
    }
}
