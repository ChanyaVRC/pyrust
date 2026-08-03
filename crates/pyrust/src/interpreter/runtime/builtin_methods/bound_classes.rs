impl Interpreter {
    fn call_class_bound_builtin_method(
        &mut self,
        receiver: &Value,
        method: &str,
        pos: &[Value],
        kw: &PyDict,
    ) -> Result<Value> {
        let ValueKind::PyClass(class) = receiver.kind() else {
            unreachable!("receiver family checked by bound method dispatcher");
        };
        // `type.mro()` — returns the MRO as a list (same entries
        // as `__mro__` tuple).  CPython's `type.mro(self)` is a
        // C slot that returns `list(self.__mro__)`.
        //
        // Two call forms:
        //   B.mro()          → bound receiver is B, pos is empty
        //   type.mro(B)      → bound receiver is `type`, pos[0] is B
        let class = Rc::clone(class);
        match method {
            "mro" => {
                // Determine the target class.  When the bound receiver
                // IS the `type` metaclass, the first positional arg is
                // the self argument (unbound descriptor form).  When
                // the receiver is any other class, mro() takes no
                // extra arguments.
                let receiver_is_type = Rc::ptr_eq(&class, &type_class_singleton());
                let target_class: Rc<RefCell<PyClass>> = if receiver_is_type {
                    // Unbound descriptor call: type.mro(B).
                    // Requires exactly one positional arg that is a type,
                    // with no extra positional or keyword arguments.
                    if pos.is_empty() {
                        return Err(pyrust_core::descriptor_needs_arg!("mro", "type", method));
                    }
                    // pos[0] is the self (type) argument.
                    let maybe_class = match pos[0].kind() {
                        ValueKind::PyClass(c) => Some(Rc::clone(c)),
                        _ => None,
                    };
                    let target = match maybe_class {
                        Some(c) => c,
                        None => {
                            let type_name = pyrust_core::builtin_type_name(&pos[0]).to_string();
                            return Err(pyrust_core::type_err!(
                                "descriptor 'mro' for 'type' objects doesn't apply to a '{type_name}' object",
                            ));
                        }
                    };
                    // After resolving self, no extra positional args or kwargs allowed.
                    reject_kwargs!(kw, "type.mro");
                    if pos.len() > 1 {
                        let extra = pos.len() - 1;
                        return Err(pyrust_core::type_err!(
                            "type.mro() takes no arguments ({extra} given)",
                        ));
                    }
                    target
                } else if pos.is_empty() && kw.is_empty() {
                    // Bound call: B.mro()
                    class
                } else if !kw.is_empty() {
                    // Keyword arguments are never accepted.
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!(
                        "{class_name}.mro() takes no keyword arguments"
                    ));
                } else {
                    // Too many positional arguments.
                    let n = pos.len();
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!(
                        "{class_name}.mro() takes no arguments ({n} given)",
                    ));
                };
                Ok(Value::list(class_mro_items(&target_class)?))
            }
            "__subclasses__" => {
                // CPython: type.__subclasses__(self) → list of direct subclasses.
                // Takes no arguments. Prunes stale weak refs lazily.
                // CPython distinguishes kw vs positional:
                //   A.__subclasses__(1)       → "takes no arguments (1 given)"
                //   A.__subclasses__(x=1)     → "takes no keyword arguments"
                //   A.__subclasses__(1, x=2)  → "takes no keyword arguments"
                let class_name = class.borrow().name.clone();
                reject_kwargs!(kw, "{class_name}.__subclasses__");
                let n_pos = pos.len();
                if n_pos > 0 {
                    return Err(pyrust_core::type_err!(
                        "{class_name}.__subclasses__() takes no arguments ({n_pos} given)",
                    ));
                }
                Ok(Value::list(class_direct_subclasses(&class)))
            }
            // Issue #1563: `dict.fromkeys` classmethod on a dict subclass.
            // `env.rs` binds the subclass as the receiver when `fromkeys` is
            // looked up on a non-primitive dict subclass.  Collect the iterable
            // and optional default, call `cls()` to construct an empty instance,
            // then replace its `__builtin_data__` with the populated dict.
            "fromkeys" => {
                reject_kwargs!(kw, "{}.fromkeys", class.borrow().name);
                if pos.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "fromkeys expected at least 1 argument, got 0"
                    ));
                }
                if pos.len() > 2 {
                    let n = pos.len();
                    return Err(pyrust_core::type_err!(
                        "fromkeys expected at most 2 arguments, got {n}",
                    ));
                }
                let default_val = pos.get(1).cloned().unwrap_or_else(Value::none);
                // Collect keys before moving pos into bound_method_pos_buf
                // so we can borrow &pos[0] without an extra clone.
                let keys = self.collect_iterable(&pos[0])?;
                // `_PyDict_FromKeys` deliberately skips its presized exact-
                // dict/set fast paths when `cls()` returned a dict subclass.
                // Build from the shared empty table so duplicate-heavy inputs
                // resize from their live-key count in the same probe order.
                let mut map = PyDict::default();
                for key in &keys {
                    let py_key = self.value_to_pykey(key)?;
                    self.dict_insert(&mut map, py_key, default_val.clone())?;
                }
                // Construct `cls()` with no arguments to get a subclass instance
                // (matching CPython's `dict.fromkeys` classmethod semantics).
                let instance = self.call_class_expanded(Rc::clone(&class), &[])?;
                // Replace the empty `__builtin_data__` backing set by
                // `call_class_expanded` with the populated dict.
                if let ValueKind::PyInstance(inst_rc) = instance.kind() {
                    inst_rc
                        .borrow_mut()
                        .attrs
                        .insert(BUILTIN_DATA_ATTR, Value::dict(map));
                }
                Ok(instance)
            }
            _ => {
                let class_name = class.borrow().name.clone();
                Err(PyError::attribute_error(
                    format!("type object '{class_name}' has no attribute '{method}'"),
                    Some(method.to_string()),
                    Some(Value::py_class(Rc::clone(&class))),
                ))
            }
        }
    }
}
