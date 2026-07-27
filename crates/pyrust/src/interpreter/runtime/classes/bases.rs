impl Interpreter {
    /// PEP 560: resolve `__mro_entries__` on non-class bases.
    ///
    /// Reads the raw base values from `R[bases_base .. bases_base+bases_n]` and,
    /// for any entry that is not a class, looks up `__mro_entries__` via
    /// a regular attribute access (CPython's `_PyObject_LookupAttr`, so instance
    /// attributes and `__getattr__` are honored — not just the type slot) and
    /// calls `entry.__mro_entries__(orig_bases)` with the *full* original bases
    /// tuple.  The returned tuple is flattened into the resolved bases.  Entries
    /// whose `__mro_entries__` lookup raises `AttributeError` are passed through
    /// unchanged so the downstream resolver still produces the proper "class
    /// base must be a class" error for genuine non-class bases.
    ///
    /// Returns `(resolved_bases, orig_bases)` where `orig_bases` is `Some` only
    /// when at least one base went through `__mro_entries__`, in which case the
    /// created class records the original bases tuple in `__orig_bases__`.
    fn resolve_class_bases_mro_entries(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        bases_base: crate::bytecode::Reg,
        bases_n: u32,
    ) -> Result<(Vec<Value>, Option<Vec<Value>>)> {
        // Read the raw bases first.
        let mut raw: Vec<Value> = Vec::with_capacity(bases_n as usize);
        for i in 0..bases_n as usize {
            let reg = (bases_base as usize + i) as crate::bytecode::Reg;
            raw.push(vm_read(regs, reg, num_locals)?);
        }

        // Fast path: every entry is already a class.
        let needs_mro_entries = raw
            .iter()
            .any(|base| !matches!(base.kind(), ValueKind::PyClass(_)));
        if !needs_mro_entries {
            return Ok((raw, None));
        }

        let orig_bases_tuple = Value::tuple(raw.clone());
        let mut resolved: Vec<Value> = Vec::with_capacity(raw.len());
        let mut used_mro_entries = false;
        for base in &raw {
            if matches!(base.kind(), ValueKind::PyClass(_)) {
                resolved.push(base.clone());
                continue;
            }
            // Non-class base: look up `__mro_entries__` via a regular attribute
            // access (CPython does `_PyObject_LookupAttr(base, ...)`), so that
            // instance attributes and `__getattr__`-served values are honored,
            // not only the type slot.  The lookup already binds any descriptor
            // (a class-method `__mro_entries__` comes back as a bound method),
            // so the result is called directly with just the original bases.
            let mro_entries_fn = match self.get_attr(base, "__mro_entries__") {
                Ok(f) => f,
                Err(ref e) if e.class_name_is("AttributeError") => {
                    // No `__mro_entries__`: pass through so the resolver raises
                    // the proper "class base must be a class" error.
                    resolved.push(base.clone());
                    continue;
                }
                Err(e) => return Err(e),
            };
            let result = self.call_function_expanded(
                mro_entries_fn,
                &[ExpandedCallArg {
                    name: None,
                    value: orig_bases_tuple.clone(),
                }],
            )?;
            let ValueKind::Tuple(entries) = result.kind() else {
                return Err(pyrust_core::type_err!(
                    "__mro_entries__ must return a tuple"
                ));
            };
            resolved.extend(entries.iter().cloned());
            used_mro_entries = true;
        }

        let orig = if used_mro_entries { Some(raw) } else { None };
        Ok((resolved, orig))
    }

    /// Compute the metaclass "winner" inherited from the bases for a `class`
    /// statement with no explicit `metaclass=`.  CPython picks the most-derived
    /// metatype among the bases (`type` is the floor).  Returns `Some(winner)`
    /// only when the winner is a *custom* metaclass (something other than the
    /// built-in `type` singleton), in which case `exec_make_class` must route
    /// through the full metaclass protocol so the custom `__new__` / `__prepare__`
    /// run.  Returns `None` when every base is plain-`type`-metatyped (the common
    /// case), keeping the native fast build.  A bases pair whose metatypes are
    /// unrelated is a metaclass conflict, matching CPython.
    fn inherited_metaclass_winner(
        &mut self,
        bases: &[Value],
    ) -> Result<Option<Rc<RefCell<PyClass>>>> {
        let mut winner: Option<Rc<RefCell<PyClass>>> = None;
        for base_val in bases {
            let ValueKind::PyClass(c) = base_val.kind() else {
                continue;
            };
            let meta = metaclass_of(c);
            // Skip the default `type` metatype — it never beats a custom winner.
            if Rc::ptr_eq(&meta, &type_class_singleton()) {
                continue;
            }
            match &winner {
                None => winner = Some(meta),
                Some(cur) => {
                    if class_is_subclass_of(&meta, cur) {
                        winner = Some(meta);
                    } else if !class_is_subclass_of(cur, &meta) {
                        return Err(pyrust_core::type_err!(
                            "metaclass conflict: the metaclass of a derived class \
                             must be a (non-strict) subclass of the metaclasses of \
                             all its bases"
                        ));
                    }
                }
            }
        }
        Ok(winner)
    }

    /// Resolve the (already `__mro_entries__`-flattened) base values into
    /// (primary, extras), rejecting non-class and non-subclassable bases, and
    /// incompatible layouts.
    fn make_class_resolve_bases(&mut self, bases: &[Value]) -> Result<ResolvedBases> {
        let mut classes: Vec<Rc<RefCell<PyClass>>> = Vec::with_capacity(bases.len());
        for base_val in bases {
            let ValueKind::PyClass(c) = base_val.kind() else {
                return Err(PyError::Runtime("class base must be a class".to_string()));
            };
            let cls = Rc::clone(c);
            if let Some(tname) = crate::interpreter::non_subclassable_builtin_name(&cls) {
                return Err(pyrust_core::type_err!(
                    "type '{tname}' is not an acceptable base type"
                ));
            }
            classes.push(cls);
        }
        // CPython's `best_base` layout check (issue #1677 for C types,
        // issue #2109 for user `__slots__`): two bases whose instance layouts
        // (solid bases) are unrelated cannot be combined.
        if crate::interpreter::bases_have_layout_conflict(&classes) {
            return Err(pyrust_core::type_err!(
                "multiple bases have instance lay-out conflict"
            ));
        }
        let mut iter = classes.into_iter();
        let base = iter.next();
        Ok((base, iter.collect()))
    }

    /// PEP 487 / CPython type_new_set_names: call `__set_name__(cls, name)` on
    /// every namespace value whose *type* defines it.
    fn make_class_call_set_name(&mut self, class: &Rc<RefCell<PyClass>>) -> Result<()> {
        let cls_val = Value::py_class(Rc::clone(class));
        let attrs_snapshot: Vec<(String, Value)> = class
            .borrow()
            .attrs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (attr_name, attr_val) in &attrs_snapshot {
            let hook_args = [
                ExpandedCallArg {
                    name: None,
                    value: cls_val.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(attr_name.clone()),
                },
            ];
            let hook_result = if let ValueKind::PyInstance(instance) = attr_val.kind() {
                let instance_class = Rc::clone(&instance.borrow().class);
                let Some(set_name) = lookup_class_attr(&instance_class, "__set_name__") else {
                    continue;
                };
                invoke_class_method(self, set_name, attr_val.clone(), &hook_args)
            } else {
                let set_name = match self.get_attr(attr_val, "__set_name__") {
                    Ok(set_name) => set_name,
                    Err(error) if error.class_name_is("AttributeError") => continue,
                    Err(error) => return Err(error),
                };
                self.call_function_expanded(set_name, &hook_args)
            };
            if let Err(e) = hook_result {
                // CPython type_new_set_names appends a note to the propagated
                // exception naming the descriptor, attribute, and owning class:
                //   "Error calling __set_name__ on 'D' instance 'd' in 'C'"
                // (3.12 re-raises the original exception with the note rather
                // than wrapping it in RuntimeError).
                let descriptor_type = value_type_name_str(attr_val);
                let owner_name = class.borrow().name.clone();
                let note = format!(
                    "Error calling __set_name__ on '{descriptor_type}' instance '{attr_name}' in '{owner_name}'"
                );
                let exc_val = self.materialize_pyerror(e)?;
                // Append `note` to the exception's `__notes__` list (PEP 678),
                // mirroring `BaseException.add_note`.
                if let ValueKind::PyInstance(exc_inst) = exc_val.kind() {
                    {
                        let mut inst = exc_inst.borrow_mut();
                        if !inst.attrs.contains_key("__notes__") {
                            inst.attrs.insert("__notes__", Value::list(vec![]));
                        }
                    };
                    if let Some(notes) = exc_inst.borrow().attrs.get_cloned("__notes__") {
                        let _ = notes.list_push(Value::string(note));
                    }
                }
                return Err(PyError::Raised(exc_val));
            }
        }
        Ok(())
    }

    /// PEP 487 / CPython type_new_init_subclass: call `base.__init_subclass__`
    /// with the class keyword arguments after the class is fully constructed.
    fn make_class_call_init_subclass(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        regs: &RegSlice,
        kwarg_base: crate::bytecode::Reg,
        class_kwarg_names: &[String],
    ) -> Result<()> {
        let kwarg_args: ExpandedArgBuf = class_kwarg_names
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let reg = (kwarg_base as usize + i) as crate::bytecode::Reg;
                ExpandedCallArg {
                    name: Some(key.clone()),
                    value: regs[reg as usize].clone(),
                }
            })
            .collect();
        self.make_class_call_init_subclass_with_kwargs(class, &kwarg_args)
    }

    /// Core of PEP 487 `__init_subclass__` dispatch shared by the `class`
    /// statement (`make_class_call_init_subclass`) and the `type()` /
    /// `type.__new__` constructor path: looks up `base.__init_subclass__` and
    /// invokes it with the already-assembled keyword arguments.
    fn make_class_call_init_subclass_with_kwargs(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        kwarg_args: &[ExpandedCallArg],
    ) -> Result<()> {
        let lookup_base = class
            .borrow()
            .base
            .clone()
            .unwrap_or_else(object_class_singleton);
        let Some(method_val) = lookup_class_attr(&lookup_base, "__init_subclass__") else {
            return Ok(());
        };
        let new_cls = Value::py_class(Rc::clone(class));
        invoke_class_method(self, method_val, new_cls, kwarg_args)?;
        Ok(())
    }
}
