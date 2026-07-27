// Function/class statement materialization and metaclass orchestration.
impl Interpreter {
    /// Build a user function value from a compiled `FnProto`.
    ///
    /// Extracted from the `MakeFunction` VM dispatch arm so that changes to
    /// function-construction semantics (annotations, defaults, etc.) only
    /// require touching this method rather than vm.rs.
    pub(crate) fn exec_make_function(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        proto_idx: u16,
        defs_base: crate::bytecode::Reg,
        annots_base: crate::bytecode::Reg,
    ) -> Result<Value> {
        let proto = code.fn_protos.get(proto_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: fn_proto index {} out of range (pool size {})",
                proto_idx,
                code.fn_protos.len()
            ))
        })?;
        let proto_code = Rc::clone(&proto.code);
        let proto_name = proto.name.clone();
        let proto_qualname = proto.qualname.clone();
        let proto_local_index = Rc::clone(&proto.local_index);
        let proto_param_binds = Rc::clone(&proto.param_binds);
        let proto_self_bind = proto.self_bind;
        let proto_local_names = Rc::clone(&proto.local_names);
        let proto_global_names = Rc::clone(&proto.global_names);
        let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
        let param_spec = Rc::clone(&proto.param_spec);
        let annotation_keys = proto.annotation_keys.clone();
        let is_memo_pure = proto.is_memo_pure;
        let proto_doc = proto.docstring.as_ref().map(|s| Value::string(s.clone()));

        let mut params = Vec::with_capacity(param_spec.names.len());
        let mut def_slot = 0u32;
        for i in 0..param_spec.names.len() {
            let default = if param_spec.has_default[i] {
                let v = vm_read(regs, defs_base + def_slot, num_locals)?;
                def_slot += 1;
                Some(v)
            } else {
                None
            };
            params.push(UserFunctionParam {
                name: param_spec.names[i].clone(),
                default,
                is_args: param_spec.is_args[i],
                is_kwargs: param_spec.is_kwargs[i],
                is_keyword_only: param_spec.is_keyword_only[i],
                is_positional_only: param_spec.is_positional_only[i],
            });
        }
        let memo_positional_parameter_count = u16::try_from(
            params
                .iter()
                .filter(|parameter| {
                    !parameter.is_args && !parameter.is_kwargs && !parameter.is_keyword_only
                })
                .count(),
        )
        .unwrap_or(u16::MAX);
        let mut annotations_map: PyDict = PyDict::default();
        for (i, key) in annotation_keys.iter().enumerate() {
            let val = vm_read(regs, annots_base + i as u32, num_locals)?;
            annotations_map.insert(PyKey::str_from(key.as_str()), val);
        }
        // #2256: don't eagerly allocate an empty dict for the (common)
        // unannotated function — store the `unset()` sentinel and let
        // `__annotations__` materialise lazily on first access.
        let annotations = if annotations_map.is_empty() {
            Value::unset()
        } else {
            Value::dict(annotations_map)
        };
        for name in proto_nonlocal_names.iter() {
            if !has_local_binding_in_current_or_ancestor(&self.env, name) {
                return Err(pyrust_core::py_err!(
                    "SyntaxError",
                    "no binding for nonlocal '{}' found",
                    name
                ));
            }
        }
        let func = Rc::new(UserFunction {
            id: crate::value::next_fn_id(),
            kind: crate::value::UserFunctionKind::Regular,
            name: proto_name,
            qualname: proto_qualname,
            name_overrides: std::cell::RefCell::new(None),
            // #2256: lazy `__main__` default — store the `unset()` sentinel so
            // the (universal) never-reassigned case carries no per-closure heap
            // `String`; `module_value()` materialises `"__main__"` on read.
            module: std::cell::RefCell::new(Value::unset()),
            doc: std::cell::RefCell::new(proto_doc.unwrap_or_else(Value::none)),
            attrs: std::cell::RefCell::new(None),
            annotations: std::cell::RefCell::new(annotations),
            // #2395: no per-object `__defaults__` / `__kwdefaults__` override
            // until user code reassigns one; the binder falls back to the
            // compile-time `params[].default` values.
            defaults_override: std::cell::RefCell::new(None),
            params,
            param_binds: proto_param_binds,
            memo_positional_parameter_count,
            self_bind: proto_self_bind,
            local_names: proto_local_names,
            local_index: proto_local_index,
            global_names: proto_global_names,
            nonlocal_names: proto_nonlocal_names,
            env: Rc::clone(&self.env),
            is_memo_pure,
            precompiled_code: Some(proto_code),
            wrapped_func: None,
        });
        Ok(Value::user_function(func))
    }

    /// Construct a class value from a compiled `FnProto` body.
    ///
    /// Extracted from the `MakeClass` VM dispatch arm so that changes to
    /// class-construction semantics (__slots__, PEP 487/695, etc.) only
    /// require touching this method rather than vm.rs.  The per-phase work is
    /// split into helpers below to keep this top-level flow readable:
    /// seed regs -> run body -> collect attrs -> resolve bases -> build class
    /// -> run PEP 487 hooks.
    // VM instruction entry: the arg list is the decoded `MakeClass` operands
    // (proto/bases/name/kwargs registers); packing them into a struct only moves
    // the operand list without simplifying the call site.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_make_class(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        proto_idx: u16,
        bases_base: crate::bytecode::Reg,
        bases_n: u32,
        name_idx: u16,
        kwarg_base: crate::bytecode::Reg,
        kwarg_n: u32,
    ) -> Result<Value> {
        let class_name = code
            .names
            .get(name_idx as usize)
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {} out of range (pool size {})",
                    name_idx,
                    code.names.len()
                ))
            })?
            .clone();
        let proto = code.fn_protos.get(proto_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: fn_proto index {} out of range (pool size {})",
                proto_idx,
                code.fn_protos.len()
            ))
        })?;
        let class_code = Rc::clone(&proto.code);
        let local_index = Rc::clone(&proto.local_index);
        // Classes (one per `def`) take an owned `String`; the `Rc<str>` sharing
        // of #2256 targets the many-closures-per-def case, not class objects.
        let proto_qualname = proto.qualname.to_string();
        let proto_global_names = Rc::clone(&proto.global_names);
        let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
        let class_docstring = proto.docstring.clone();
        let class_kwarg_names = proto.class_kwarg_names.clone();

        // PEP 560: resolve `__mro_entries__` on any non-class base before any
        // other base inspection.  `resolved_bases` is the flattened list that
        // drives the metaclass winner, the MRO, and `type.__new__`; `orig_bases`
        // is `Some` only when at least one base went through `__mro_entries__`,
        // in which case the new class records `__orig_bases__`.
        let (resolved_bases, orig_bases) =
            self.resolve_class_bases_mro_entries(regs, num_locals, bases_base, bases_n)?;

        // No explicit `metaclass=`, but a base may carry a custom metatype that
        // must drive class creation (CPython computes the "winner" metaclass as
        // the most-derived metatype of the bases).  Detect that here, before
        // running the body, and route through the full metaclass protocol
        // (`__prepare__` → body → `metaclass(name, bases, ns, **kw)`) so e.g.
        // `class Color(Enum)` invokes `EnumMeta.__new__`.  When every base is a
        // plain `type`-metatyped class (the common case), fall through to the
        // fast native build below.
        if let Some(winner) = self.inherited_metaclass_winner(&resolved_bases)? {
            let bases_tuple = Value::tuple(resolved_bases);
            let kwargs: ExpandedArgBuf = class_kwarg_names
                .iter()
                .take(kwarg_n as usize)
                .enumerate()
                .map(|(i, key)| {
                    let reg = (kwarg_base as usize + i) as crate::bytecode::Reg;
                    ExpandedCallArg {
                        name: Some(key.clone()),
                        value: regs[reg as usize].clone(),
                    }
                })
                .collect();
            return self.make_class_via_metaclass(
                Value::py_class(winner),
                class_name,
                bases_tuple,
                kwargs,
                class_code,
                local_index,
                proto_qualname,
                proto_global_names,
                proto_nonlocal_names,
                orig_bases,
            );
        }

        // Run the class body, collecting the attrs dict it stores.
        let (mut attrs, class_env_rc) = self.run_class_body(
            &class_code,
            &local_index,
            &proto_qualname,
            proto_global_names,
            proto_nonlocal_names,
        )?;

        // Adjust the attrs dict per CPython's type_new rules, returning the
        // resolved __qualname__.
        let qualname =
            make_class_finalize_attrs(&mut attrs, proto_qualname, class_docstring.as_deref())?;

        // Resolve and validate the base classes (already `__mro_entries__`-
        // flattened above).
        let (base, extra_bases_vec) = self.make_class_resolve_bases(&resolved_bases)?;

        // PEP 560: when any base went through `__mro_entries__`, record the
        // *original* bases tuple in `__orig_bases__` (set before the class is
        // built so it is part of the namespace, matching CPython).
        if let Some(orig) = orig_bases {
            attrs.insert("__orig_bases__".to_string(), Value::tuple(orig));
        }

        let class_kwargs: ExpandedArgBuf = class_kwarg_names
            .iter()
            .take(kwarg_n as usize)
            .enumerate()
            .map(|(index, name)| {
                let reg = (kwarg_base as usize + index) as crate::bytecode::Reg;
                ExpandedCallArg {
                    name: Some(name.clone()),
                    value: regs[reg as usize].clone(),
                }
            })
            .collect();
        if let Some(class) = self.try_build_builtin_class_adapter(
            &class_name,
            &attrs,
            base.as_ref(),
            &extra_bases_vec,
            &class_kwargs,
        )? {
            return Ok(class);
        }

        let slots = make_class_extract_slots(&mut attrs)?;
        let class = Rc::new(RefCell::new(PyClass {
            extra_bases: extra_bases_vec.clone(),
            slots,
            ..PyClass::new(class_name, qualname, base.clone(), attrs)
        }));
        install_slot_member_descriptors(&class);
        class_mro_items(&class).map(|_| ())?;

        // Register as a subclass of every base, and seed the __class__ cell.
        if let Some(ref b) = base {
            b.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&class));
        }
        for eb in &extra_bases_vec {
            eb.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&class));
        }
        class_env_rc
            .borrow_mut()
            .values
            .insert("__class__", Value::py_class(Rc::clone(&class)));

        // PEP 487 hooks: __set_name__ on every descriptor, then
        // __init_subclass__ on the base.
        self.make_class_call_set_name(&class)?;
        self.make_class_call_init_subclass(&class, regs, kwarg_base, &class_kwarg_names)?;
        Ok(Value::py_class(class))
    }

    /// Construct a class through an explicit `metaclass=` (issue #2128/#2130).
    ///
    /// Mirrors CPython's `class` statement protocol for the metaclass path:
    ///   1. `ns = metaclass.__prepare__(name, bases, **kwds)`,
    ///   2. run the class body, collecting its assignments,
    ///   3. populate `ns` (`__module__`, `__qualname__`, then the body's
    ///      assignments in order) — going through `__setitem__` so a custom
    ///      recording mapping observes them,
    ///   4. `metaclass(name, bases_tuple, ns, **kwds)`.
    ///
    /// All class-creation hooks (`__set_name__`, `__init_subclass__`) then run
    /// once inside the metaclass call (`type.__new__` → `build_class_via_type`),
    /// not in this method.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_make_class_meta(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        proto_idx: u16,
        bases_base: crate::bytecode::Reg,
        bases_n: u32,
        name_idx: u16,
        kwarg_base: crate::bytecode::Reg,
        kwarg_n: u32,
        meta_reg: crate::bytecode::Reg,
    ) -> Result<Value> {
        let class_name = code
            .names
            .get(name_idx as usize)
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {} out of range (pool size {})",
                    name_idx,
                    code.names.len()
                ))
            })?
            .clone();
        let proto = code.fn_protos.get(proto_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: fn_proto index {} out of range (pool size {})",
                proto_idx,
                code.fn_protos.len()
            ))
        })?;
        let class_code = Rc::clone(&proto.code);
        let local_index = Rc::clone(&proto.local_index);
        // Classes (one per `def`) take an owned `String`; the `Rc<str>` sharing
        // of #2256 targets the many-closures-per-def case, not class objects.
        let proto_qualname = proto.qualname.to_string();
        let proto_global_names = Rc::clone(&proto.global_names);
        let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
        let class_kwarg_names = proto.class_kwarg_names.clone();

        let metaclass = vm_read(regs, meta_reg, num_locals)?;

        // PEP 560: resolve `__mro_entries__` on any non-class base before the
        // metaclass protocol runs (CPython's `__build_class__` resolves bases
        // for every class statement, including those with an explicit
        // `metaclass=`).  The resolved tuple drives `__prepare__` and the
        // metaclass call; `orig_bases` is `Some` only when a base was
        // substituted, recording `__orig_bases__` in the namespace.
        let (resolved_bases, orig_bases) =
            self.resolve_class_bases_mro_entries(regs, num_locals, bases_base, bases_n)?;
        let bases_tuple = Value::tuple(resolved_bases);

        // Assemble the class keyword arguments (everything except `metaclass`,
        // which is not part of `keywords`).  Forwarded to both __prepare__ and
        // the metaclass call.
        let kwargs: ExpandedArgBuf = class_kwarg_names
            .iter()
            .take(kwarg_n as usize)
            .enumerate()
            .map(|(i, key)| {
                let reg = (kwarg_base as usize + i) as crate::bytecode::Reg;
                ExpandedCallArg {
                    name: Some(key.clone()),
                    value: regs[reg as usize].clone(),
                }
            })
            .collect();

        self.make_class_via_metaclass(
            metaclass,
            class_name,
            bases_tuple,
            kwargs,
            class_code,
            local_index,
            proto_qualname,
            proto_global_names,
            proto_nonlocal_names,
            orig_bases,
        )
    }
}
