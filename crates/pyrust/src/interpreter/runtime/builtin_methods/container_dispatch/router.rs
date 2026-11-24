/// Built-in container receivers handled by every method-call entry path.
///
/// Keeping this as a type prevents the VM/call boundary from passing magic
/// numeric tags and makes adding a new fast-path receiver an exhaustive-match
/// change.
#[derive(Clone, Copy)]
pub(super) enum BuiltinContainerKind {
    List,
    Dict,
    Tuple,
    Str,
    Bytes,
    Set,
}

impl BuiltinContainerKind {
    #[inline(always)]
    pub(super) fn classify(value: Option<&Value>) -> Option<Self> {
        match value?.kind() {
            ValueKind::List(_) => Some(Self::List),
            ValueKind::Dict(_) => Some(Self::Dict),
            ValueKind::Tuple(_) => Some(Self::Tuple),
            ValueKind::Str(_) => Some(Self::Str),
            ValueKind::Bytes(_) => Some(Self::Bytes),
            ValueKind::Set(_) => Some(Self::Set),
            _ => None,
        }
    }

    pub(super) fn from_type_name(type_name: &str) -> Option<Self> {
        match type_name {
            "list" => Some(Self::List),
            "dict" => Some(Self::Dict),
            "tuple" => Some(Self::Tuple),
            "str" => Some(Self::Str),
            "bytes" => Some(Self::Bytes),
            "set" => Some(Self::Set),
            _ => None,
        }
    }

    /// Whether this receiver kind owns `method` in the direct instance-method
    /// dispatcher.
    ///
    /// `bytes.fromhex` is a classmethod and `bytes.maketrans` is a static
    /// helper; CPython also exposes both through an instance, but their binding
    /// semantics belong to the primitive-class descriptor path.  Keep those
    /// names out of the fused exact-bytes path instead of duplicating their
    /// class/static binding rules here.
    #[inline(always)]
    pub(super) fn supports_direct_method(self, method: &str) -> bool {
        !matches!(self, Self::Bytes) || pyrust_builtins::bytes::has_method(method)
    }
}

impl Interpreter {
    /// Dispatch a builtin method recovered from the generic `CallMethod`
    /// inline cache.
    ///
    /// The cache owns class/version validation only. Exact primitive method
    /// names and backing-storage policy stay in this semantic domain, so the
    /// fast-path module does not decode builtin sentinels or
    /// `__builtin_data__` itself.
    pub(super) fn try_dispatch_cached_builtin_method(
        &mut self,
        unbound: &Value,
        instance: &Rc<RefCell<pyrust_core::PyInstance>>,
        args: &[Value],
    ) -> Option<Result<Value>> {
        let ValueKind::BuiltinFunction(function_name) = unbound.kind() else {
            return None;
        };
        let (primitive_type, primitive_method) = function_name
            .split_once('.')
            .filter(|(type_name, _)| is_mutable_container_builtin_type_name(type_name))?;
        let receiver = Value::py_instance(Rc::clone(instance));
        let backing = effective_builtin_receiver(&receiver, &[])?;
        let ordered = is_ordered_dict_class_or_subclass(&instance.borrow().class);
        if primitive_method.starts_with("__")
            && is_protocol_dunder(primitive_type, primitive_method)
        {
            if is_mutable_builtin_inplace_dunder(primitive_method) {
                return Some(self.dispatch_builtin_subclass_protocol_dunder(
                    primitive_method,
                    &receiver,
                    backing,
                    args.to_vec(),
                ));
            }
            return Some(self.dispatch_builtin_protocol_dunder(
                primitive_method,
                backing,
                args.to_vec(),
            ));
        }
        if primitive_type == "dict"
            && let Some(spec) = pyrust_builtins::dict::method_spec(primitive_method)
            && spec.view_method().is_some()
        {
            if let Err(err) = spec.validate_positional_arity(args.len()) {
                return Some(Err(err));
            }
            return Some(Self::dict_view_for_owner(
                &backing,
                spec.method().name(),
                ordered,
                Value::py_instance(Rc::clone(instance)),
            ));
        }
        Some(self.dispatch_backing_primitive_method(
            primitive_type,
            primitive_method,
            backing,
            ordered,
            args.to_vec(),
        ))
    }

    /// Allocation-free, execution-free 1-arg fast path for hot builtin
    /// container mutators (`set.add(int|str)`), called before `exec_call_method`
    /// allocates `Vec<Value>` and an empty kwargs `PyDict` (#2924).
    #[inline]
    pub(super) fn try_dispatch_builtin_container_method_1arg(
        kind: BuiltinContainerKind,
        receiver: &Value,
        method: &str,
        arg: Value,
    ) -> Result<std::result::Result<Value, Value>> {
        if matches!(kind, BuiltinContainerKind::Set)
            && pyrust_builtins::set::method_spec(method)
                .is_some_and(|spec| matches!(spec.method(), pyrust_builtins::set::Method::Add))
        {
            if let Some(i) = arg.as_int() {
                receiver.set_add(PyKey::Int(i))?;
                return Ok(Ok(Value::none()));
            }
            if arg.is_str() {
                receiver.set_add(PyKey::Str(arg))?;
                return Ok(Ok(Value::none()));
            }
        }
        Ok(Err(arg))
    }

    /// Shared dispatch body for method calls on the six classified builtin
    /// container types (`list` / `dict` / `tuple` / `str` / `bytes` / `set`). Both
    /// `Insn::CallMethod` (no kwargs) and `Insn::CallMethodExpanded`
    /// (kwargs / unpacking) route here once they have materialised the
    /// receiver, positional args, and keyword map — so the dispatch decision
    /// lives in exactly one place (#431).
    ///
    /// The "needs interpreter" vs "needs Rc backing" vs "pure builtin"
    /// decision is route-driven via the `pyrust-builtins::<type>` modules
    /// (`list::interpreter_method`, `dict::view_method`,
    /// `string::interpreter_method`) — the single source of truth documented
    /// in `crates/pyrust-builtins/README.md`.
    pub(super) fn dispatch_builtin_container_method(
        &mut self,
        kind: BuiltinContainerKind,
        receiver: Value,
        method: &str,
        pos: smallvec::SmallVec<[Value; 4]>,
        kw: &PyDict,
        ordered_mapping: bool,
    ) -> Result<Value> {
        // All entry paths — bytecode method opcodes, bound-method values, and
        // unbound type descriptors — converge here.  Keeping `__iter__` here
        // avoids a fourth, name-based routing decision in each caller.
        if method == "__iter__" {
            reject_kwargs!(kw, "wrapper __iter__");
            if !pos.is_empty() {
                return Err(pyrust_core::type_err!(
                    "expected 0 arguments, got {}",
                    pos.len()
                ));
            }
            let iter_arg = ExpandedCallArg {
                name: None,
                value: receiver,
            };
            let dispatch =
                crate::builtin_registry::lookup("iter").expect("iter must be in the registry");
            return dispatch(self, &[iter_arg]);
        }

        // Issue #2151: object-protocol methods (`__sizeof__`/`__dir__`/
        // `__reduce__`/`__reduce_ex__`) called directly via the method-call
        // opcode on a container.  Handled here (the per-type `call` below would
        // leak a RuntimeError) so `[1].__dir__()` returns a list.
        if method.starts_with("__") {
            if crate::interpreter::is_object_protocol_method(&receiver, method) {
                return self.call_object_protocol_method(method, &receiver, &pos, !kw.is_empty());
            }
            // Issue #2191: `__format__` called directly via the method-call
            // opcode on a classified container (`"hi".__format__('>5')`,
            // `[1,2].__format__('')`).  Route through `apply_format_spec` so the
            // result matches `format(receiver, spec)` — otherwise the per-type
            // `call` below leaks a RuntimeError (`'str' object has no attribute
            // '__format__'`).  Gated under the `__` prefix so the common
            // method-name path (`.append`, `.upper`, …) is untouched.
            if method == "__format__" {
                if !kw.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "{}.__format__() takes no keyword arguments",
                        format_dunder_owner(&receiver)
                    ));
                }
                let spec = format_dunder_spec_arg(&receiver, &pos)?;
                return apply_format_spec(&receiver, spec);
            }
        }
        // Issue #1909: container/sequence protocol dunders called directly via
        // the `obj.__getitem__(i)` method-call opcode (not through the
        // bound-method value).  Route through the shared dispatcher so the
        // result matches the operator behaviour.  `__iter__` is handled by the
        // callers before this point.
        if is_container_protocol_dunder_name(method) {
            let type_name = pyrust_core::builtin_type_name(&receiver);
            if is_protocol_dunder(&type_name, method) {
                if !kw.is_empty() {
                    // Issue #2398: CPython's keyword-rejection wording depends on
                    // whether the slot is a named method-wrapper or an anonymous
                    // slot wrapper (issue #2291).
                    return Err(if is_named_protocol_wrapper(method, &type_name) {
                        pyrust_core::type_err!("{type_name}.{method}() takes no keyword arguments")
                    } else {
                        pyrust_core::type_err!("wrapper {method}() takes no keyword arguments")
                    });
                }
                return self.dispatch_builtin_protocol_dunder(method, receiver, pos.into_vec());
            }
            // Issue #2070: `__hash__` on an unhashable built-in (list/dict/set/
            // bytearray) resolves to `None` in CPython, so `[1].__hash__()`
            // raises `'NoneType' object is not callable` — not `AttributeError`.
            if method == "__hash__" && is_unhashable_builtin_type_name(&type_name) {
                return Err(pyrust_core::type_err!("'NoneType' object is not callable"));
            }
            // A protocol-dunder name that is valid for *other* types but not
            // this one (e.g. `set().__getitem__`, `"".__setitem__`) is
            // genuinely absent: raise a proper `AttributeError` rather than
            // letting the per-type `call` leak a `RuntimeError` (issue #1909).
            return Err(PyError::attribute_error(
                format!("'{type_name}' object has no attribute '{method}'"),
                Some(method.to_string()),
                Some(receiver),
            ));
        }
        match kind {
            BuiltinContainerKind::List => {
                self.dispatch_list_container_method(receiver, method, pos.into_vec(), kw)
            }
            BuiltinContainerKind::Dict => {
                self.dispatch_dict_container_method(receiver, method, pos, kw, ordered_mapping)
            }
            BuiltinContainerKind::Tuple => {
                self.dispatch_tuple_container_method(receiver, method, pos.into_vec(), kw)
            }
            BuiltinContainerKind::Str => {
                self.dispatch_str_container_method(receiver, method, pos, kw)
            }
            BuiltinContainerKind::Bytes => {
                self.dispatch_bytes_container_method(receiver, method, pos, kw)
            }
            BuiltinContainerKind::Set => {
                self.dispatch_set_container_method(receiver, method, pos.into_vec(), kw)
            }
        }
    }

    /// Dispatch a method call on the backing primitive of a `list`/`dict`/`set`
    /// subclass instance (#976).  The cached method is a
    /// `BuiltinFunction("<type>.<method>")` and `backing` is its
    /// `__builtin_data__` value; route to the same per-type path the bare
    /// container would take.  `index`/`count` on a list backing may fire user
    /// `__eq__`, so they go through the interpreter-aware sequence helpers.
    fn dispatch_backing_primitive_method(
        &mut self,
        prim_type: &str,
        prim_method: &str,
        backing: Value,
        ordered: bool,
        args: Vec<Value>,
    ) -> Result<Value> {
        let kind = BuiltinContainerKind::from_type_name(prim_type)
            .unwrap_or_else(|| unreachable!("bad backing primitive type {prim_type}"));
        self.dispatch_builtin_container_method(
            kind,
            backing,
            prim_method,
            args.into(),
            &PyDict::default(),
            ordered,
        )
    }
}
