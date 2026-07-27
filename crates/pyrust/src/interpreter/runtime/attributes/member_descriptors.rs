// Member-descriptor protocol semantics.
/// Validate a member descriptor receiver against its canonical owner identity.
///
/// The owner name is presentation-only. A user class with the same name is
/// unrelated and must be rejected; a genuine subclass is accepted even after
/// the owner's mutable `__name__` changes.
fn member_descriptor_check_receiver(
    slot: &str,
    owner_name: &str,
    owner: Option<&Rc<RefCell<PyClass>>>,
    obj: &Value,
) -> Result<()> {
    if let (Some(owner), ValueKind::PyInstance(inst)) = (owner, obj.kind()) {
        let actual_class = Rc::clone(&inst.borrow().class);
        if class_is_subclass_of(&actual_class, owner) {
            return Ok(());
        }
    }
    let actual_name = pyrust_core::builtin_type_name(obj);
    Err(pyrust_core::type_err!(
        "descriptor '{slot}' for '{owner_name}' objects doesn't apply to a '{actual_name}' object"
    ))
}

impl Interpreter {
    /// Dispatch a directly-invoked `member_descriptor` descriptor-protocol
    /// method (`S.x.__get__(obj[, owner])` / `S.x.__set__(obj, v)` /
    /// `S.x.__delete__(obj)`), issue #2084.  Mirrors CPython's
    /// `member_get`/`member_set`/`member_delete` arity and behaviour.
    pub(crate) fn member_descriptor_protocol_call(
        &mut self,
        descriptor: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        let info = pyrust_builtins::member_descriptor::as_member_descriptor_full(&descriptor)
            .expect("member_descriptor receiver");
        // CPython rejects an instance that is not of the owning type (or a
        // subclass) with a `descriptor '<slot>' for '<owner>' objects doesn't
        // apply to a '<T>' object` TypeError.  A `None` instance (class-level
        // `__get__`) is allowed and handled below.  Checking only "is a
        // PyInstance" is insufficient: an instance of an *unrelated* class is
        // not of the owning type, so it must raise the same TypeError (and in
        // particular `__set__` must not silently write into an unrelated
        // instance's storage).
        match method {
            "__get__" => {
                // CPython's wrapper messages carry a leading space (the empty
                // method-name prefix) and split too-few / too-many wording.
                if args.is_empty() {
                    return Err(pyrust_core::type_err!(
                        " expected at least 1 argument, got 0"
                    ));
                }
                if args.len() > 2 {
                    return Err(pyrust_core::type_err!(
                        " expected at most 2 arguments, got {}",
                        args.len()
                    ));
                }
                // Class-level access (`obj is None`) returns the descriptor.
                if args[0].is_none() {
                    return Ok(
                        pyrust_builtins::member_descriptor::export_member_descriptor(&descriptor)
                            .unwrap_or(descriptor),
                    );
                }
                member_descriptor_check_receiver(
                    &info.attr_name,
                    &info.owner_name,
                    info.owner.as_ref(),
                    &args[0],
                )?;
                member_descriptor_get(&args[0], info.slot_id, &info.attr_name)
            }
            "__set__" => {
                if args.len() != 2 {
                    return Err(pyrust_core::type_err!(
                        " expected 2 arguments, got {}",
                        args.len()
                    ));
                }
                member_descriptor_check_receiver(
                    &info.attr_name,
                    &info.owner_name,
                    info.owner.as_ref(),
                    &args[0],
                )?;
                match call_descriptor_set(
                    self,
                    &descriptor,
                    args[0].clone(),
                    args[1].clone(),
                    &info.attr_name,
                )? {
                    Some(r) => r.map(|_| Value::none()),
                    None => Ok(Value::none()),
                }
            }
            "__delete__" => {
                if args.len() != 1 {
                    return Err(pyrust_core::type_err!(
                        "expected 1 argument, got {}",
                        args.len()
                    ));
                }
                member_descriptor_check_receiver(
                    &info.attr_name,
                    &info.owner_name,
                    info.owner.as_ref(),
                    &args[0],
                )?;
                match call_descriptor_delete(self, &descriptor, args[0].clone(), &info.attr_name)? {
                    Some(r) => r.map(|_| Value::none()),
                    None => Ok(Value::none()),
                }
            }
            _ => unreachable!("member_descriptor_protocol_call method {method}"),
        }
    }
}
