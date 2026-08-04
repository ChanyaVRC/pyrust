impl Interpreter {
    /// Core of the explicit-metaclass class-creation protocol, shared by the
    /// `metaclass=` keyword path (`exec_make_class_meta`) and the
    /// inherited-metaclass path (`exec_make_class`, when a base carries a custom
    /// metatype).  Given an already-resolved metaclass value, this runs
    /// CPython's `__prepare__` → run body → `metaclass(name, bases, ns, **kw)`
    /// sequence so all class-creation hooks fire once inside the metaclass call.
    #[allow(clippy::too_many_arguments)]
    fn make_class_via_metaclass(
        &mut self,
        metaclass: Value,
        class_name: String,
        bases_tuple: Value,
        kwargs: ExpandedArgBuf,
        class_code: Rc<crate::bytecode::FnCode>,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        proto_qualname: String,
        proto_global_names: Rc<HashSet<String>>,
        proto_nonlocal_names: Rc<HashSet<String>>,
        orig_bases: Option<Vec<Value>>,
    ) -> Result<Value> {
        // 1. ns = metaclass.__prepare__(name, bases, **kwds).  __prepare__ is a
        //    classmethod resolved via the metaclass; `type.__prepare__` returns
        //    a fresh dict, so a metaclass without an override still gets a dict.
        //    A non-class callable metaclass (e.g. a plain function) has no
        //    `__prepare__` attribute at all — CPython falls back to a plain dict
        //    in that case (PEP 3115), so swallow the AttributeError here.
        let namespace = match self.get_attr(&metaclass, "__prepare__") {
            Ok(prepare_fn) => {
                let mut prepare_args: ExpandedArgBuf = smallvec![
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(class_name.clone())
                    },
                    ExpandedCallArg {
                        name: None,
                        value: bases_tuple.clone()
                    },
                ];
                prepare_args.extend(kwargs.iter().cloned());
                self.call_function_expanded(prepare_fn, &prepare_args)?
            }
            Err(ref e) if e.class_name_is("AttributeError") => Value::dict(PyDict::default()),
            Err(e) => return Err(e),
        };

        // 2. Run the class body and collect its ordered assignments.
        let ClassBodyExecution {
            attrs,
            class_env: class_env_rc,
            annotation_scopes,
        } = self.run_class_body(
            &class_code,
            &local_index,
            &proto_qualname,
            proto_global_names,
            proto_nonlocal_names,
        )?;

        // 3. Populate the namespace in CPython's order: `__module__` first,
        //    `__qualname__` second (the latter is intercepted out of the body
        //    attrs by run_class_body), then the body's assignments in the order
        //    they ran (skipping the `__module__` already emitted).  Set via a
        //    generic setitem so a custom mapping's __setitem__ records them.
        if let Some(module_val) = attrs.get("__module__") {
            self.namespace_set_item(&namespace, "__module__", module_val.clone())?;
        }
        self.namespace_set_item(&namespace, "__qualname__", Value::string(proto_qualname))?;
        for (k, v) in &attrs {
            if k == "__module__" {
                continue;
            }
            self.namespace_set_item(&namespace, k, v.clone())?;
        }

        // PEP 560: when any base went through `__mro_entries__`, record the
        // *original* bases tuple in `__orig_bases__` so the metaclass sees it in
        // the namespace (CPython sets it in `__build_class__` before the
        // metaclass call).
        if let Some(orig) = orig_bases {
            self.namespace_set_item(&namespace, "__orig_bases__", Value::tuple(orig))?;
        }
        Self::bind_class_annotation_mapping(&annotation_scopes, &namespace);

        // 4. metaclass(name, bases_tuple, namespace, **kwds).
        let mut call_args: ExpandedArgBuf = smallvec![
            ExpandedCallArg {
                name: None,
                value: Value::string(class_name)
            },
            ExpandedCallArg {
                name: None,
                value: bases_tuple
            },
            ExpandedCallArg {
                name: None,
                value: namespace
            },
        ];
        call_args.extend(kwargs);
        let result = self.call_function_expanded(metaclass, &call_args)?;
        if let ValueKind::PyClass(class) = result.kind() {
            Self::bind_class_annotation_owner(&annotation_scopes, class);
        }

        // Seed the `__class__` cell the body's methods closed over so that
        // zero-arg `super()` inside them resolves to the *final* class the
        // metaclass produced (mirrors the `MakeClass` path).  `class_env_rc` is
        // the env captured by every method defined in the body.
        class_env_rc
            .borrow_mut()
            .values
            .insert("__class__", result.clone());
        Ok(result)
    }

    /// Set `ns[key] = value` on a class-body namespace mapping returned by
    /// `__prepare__`.  Fast-paths a plain `dict`; otherwise dispatches to the
    /// mapping's `__setitem__` so a custom recording namespace observes the
    /// assignment (issue #2128).
    fn namespace_set_item(&mut self, ns: &Value, key: &str, value: Value) -> Result<()> {
        if ns.is_dict() {
            ns.dict_insert(PyKey::str_from(key), value)?;
            return Ok(());
        }
        let ValueKind::PyInstance(inst) = ns.kind() else {
            // BuiltinObject mappings without a user __setitem__ fall back to a
            // direct dict_insert when they expose one; otherwise this is a
            // namespace type we do not support — report it like CPython.
            if ns.dict_insert(PyKey::str_from(key), value.clone()).is_ok() {
                return Ok(());
            }
            let tname = pyrust_core::builtin_type_name(ns);
            return Err(pyrust_core::type_err!(
                "'{tname}' object does not support item assignment"
            ));
        };
        let class = Rc::clone(&inst.borrow().class);
        let Some(method_val) = lookup_class_attr(&class, "__setitem__") else {
            let class_name = class.borrow().name.clone();
            return Err(pyrust_core::type_err!(
                "'{class_name}' object does not support item assignment"
            ));
        };
        invoke_class_method(
            self,
            method_val,
            ns.clone(),
            &[
                ExpandedCallArg {
                    name: None,
                    value: Value::string(key),
                },
                ExpandedCallArg { name: None, value },
            ],
        )?;
        Ok(())
    }

    /// Run a class body and return its assembled attrs dict plus the class env
    /// (so the caller can seed `__class__` after the class object exists).
    fn run_class_body(
        &mut self,
        class_code: &Rc<crate::bytecode::FnCode>,
        local_index: &Rc<HashMap<String, crate::bytecode::Reg>>,
        proto_qualname: &str,
        proto_global_names: Rc<HashSet<String>>,
        proto_nonlocal_names: Rc<HashSet<String>>,
    ) -> Result<ClassBodyExecution> {
        let num_class_regs = class_code.num_regs as usize;
        let mut class_regs: RegsBuf = smallvec![Value::unset(); num_class_regs];
        // CPython pre-injects __qualname__ / __module__ / __annotations__ into
        // the class namespace before the body runs; seed those register slots.
        let qualname_slot = local_index.get("__qualname__").copied();
        let module_slot = local_index.get("__module__").copied();
        let annotations_slot = local_index.get("__annotations__").copied();
        seed_class_reg(&mut class_regs, qualname_slot, || {
            Value::string(proto_qualname)
        });
        // CPython sets the class body's pre-injected `__module__` to the value
        // of the global `__name__` (the compiler emits `__module__ = __name__`).
        // Resolve it from the current module namespace so classes defined inside
        // `@inject`-backed Python body files (e.g. `typing_py.py`) report the
        // correct module (`typing`, `dataclasses`, …) rather than `__main__`
        // (issue #2801).  A top-level script's module env seeds `__name__` to
        // `"__main__"`, preserving the previous default there.
        let module_name = Value::string(self.defining_module_name()?);
        seed_class_reg(&mut class_regs, module_slot, || module_name.clone());
        seed_class_reg(&mut class_regs, annotations_slot, || {
            Value::dict(PyDict::default())
        });
        // __module__ and __annotations__ always flow into the attrs dict;
        // __qualname__ is intercepted in get_attr (issue #553) so it is not
        // pre-ordered here.
        let mut pre_order: Vec<crate::bytecode::Reg> = Vec::new();
        pre_order.extend(module_slot);
        pre_order.extend(annotations_slot);
        self.class_store_order.push(pre_order);
        let has_type_alias = class_code
            .insns
            .iter()
            .any(|insn| matches!(insn, crate::bytecode::Insn::MakeTypeAlias(..)));
        self.push_class_annotation_scopes(
            local_index,
            num_class_regs,
            has_type_alias,
            qualname_slot,
        );

        // Push a class env so methods capture __class__ (zero-arg super), and a
        // Class frame view so locals() inside the body sees the namespace.
        let class_env = self.alloc_env(Some(Rc::clone(&self.env)));
        {
            let mut e = class_env.borrow_mut();
            e.global_names = proto_global_names;
            e.nonlocal_names = proto_nonlocal_names;
        }
        let class_env_rc = Rc::clone(&class_env);
        let previous_env = std::mem::replace(&mut self.env, class_env);
        let class_regs_ptr = unsafe { std::ptr::NonNull::new_unchecked(class_regs.as_mut_ptr()) };
        let class_regs_len = class_regs.len();
        self.vm_frame_views.push(VmFrameView {
            kind: FrameKind::Class,
            // SAFETY: SmallVec/Vec allocation is always non-null.  `class_regs`
            // lives on this stack frame for the full duration of `run_bytecode`;
            // the view is popped before `class_regs` is dropped.
            regs_ptr: class_regs_ptr,
            regs_len: class_regs_len,
            local_index: Rc::clone(local_index),
            nonlocal_names: None,
            env: None,
            is_class_method: false,
            function: None,
            gen_frame: None,
        });
        // SAFETY: class_regs_ptr is valid for class_regs_len Values for the
        // lifetime of class_regs.  No &mut [Value] referencing class_regs is
        // held while the dispatch loop runs (issue #547, PR #646).
        let class_regs_slice =
            unsafe { RegSlice::from_raw(class_regs_ptr.as_ptr(), class_regs_len) };
        let body_result = self.run_bytecode(class_code, class_regs_slice);
        // Always pop both stacks, even on error, to keep them balanced.
        self.vm_frame_views.pop();
        self.env = previous_env;
        self.free_env(class_env_rc.clone());
        let store_order = self
            .class_store_order
            .pop()
            .expect("class_store_order stack popped to empty");
        let (annotation_scopes, live_namespace) = self.pop_class_annotation_scopes();
        body_result?;

        let attrs = live_namespace.as_ref().map_or_else(
            || collect_class_attrs(local_index, &class_regs, store_order, num_class_regs),
            collect_live_class_attrs,
        );
        Ok(ClassBodyExecution {
            attrs,
            class_env: class_env_rc,
            annotation_scopes,
        })
    }
}

/// Convert a materialized class-frame namespace into the runtime's string-keyed
/// class attrs while preserving the live dict's insertion order.
fn collect_live_class_attrs(namespace: &Value) -> IndexMap<String, Value> {
    namespace
        .dict_with(|dict| {
            dict.iter()
                .filter_map(|(key, value)| match key {
                    PyKey::Str(name) => name.as_str().map(|name| (name.to_string(), value.clone())),
                    _ => None,
                })
                .collect()
        })
        .expect("live class namespace is a dict")
}
