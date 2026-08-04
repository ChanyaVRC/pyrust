// globals/locals namespace operations.
impl Interpreter {
    /// Resolve a cell/free-variable read, including its CPython error class and
    /// the module/builtins fallback for a deleted binding.
    pub(crate) fn resolve_cell_value(
        &mut self,
        code: &crate::bytecode::FnCode,
        name: &str,
    ) -> Result<Value> {
        let is_cell_local = code.cell_vars.iter().any(|cell| cell == name);
        let is_free = !is_cell_local && comp_read_is_free(code, name);
        let namespace = self.prepare_global_namespace(name);
        match self.lookup_name_inner(name, is_free)? {
            Some(value) => Ok(value),
            None => self.resolve_cell_miss(name, &namespace),
        }
    }

    /// Validate a fast-local read without exposing scope-specific name errors
    /// to the opcode dispatcher.
    pub(crate) fn check_local_binding(&self, value: &Value, name: &str) -> Result<()> {
        if !value.is_unset() {
            return Ok(());
        }
        // Error classification follows the active Python frame, not VM
        // implementation metadata such as a function id.  Generator frames do
        // not retain that id across suspension, but they do publish the same
        // Function frame view as ordinary and trampolined calls.
        let in_function = self
            .vm_frame_views
            .last()
            .is_some_and(|view| view.kind == FrameKind::Function);
        if in_function {
            Err(PyError::name_error(
                "UnboundLocalError",
                format!(
                    "cannot access local variable '{name}' where it is not associated with a value"
                ),
                None,
            ))
        } else {
            Err(PyError::name_error(
                "NameError",
                format!("name '{name}' is not defined"),
                Some(name.to_string()),
            ))
        }
    }

    /// Enter the lexical scope that owns PEP 695 type parameters.
    pub(crate) fn push_type_parameter_scope(&mut self) {
        self.env = self.alloc_env(Some(Rc::clone(&self.env)));
        // A type parameter may shadow a cached enclosing global.
        bump_global_struct_version(self);
    }

    /// Leave the active PEP 695 type-parameter scope.
    pub(crate) fn pop_type_parameter_scope(&mut self) {
        let parent = self
            .env
            .borrow()
            .parent
            .clone()
            .expect("PopTypeParamEnv without a matching PushTypeParamEnv");
        let scope = std::mem::replace(&mut self.env, parent);
        self.free_env(scope);
        bump_global_struct_version(self);
    }

    /// Delete a fast-local binding with scope-correct error and finalizer
    /// semantics.
    pub(crate) fn delete_local_binding(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut RegSlice,
        register: crate::bytecode::Reg,
        name_index: u16,
    ) -> Result<()> {
        let frame_kind = self.vm_frame_views.last().map(|view| view.kind);
        let is_module_scope = frame_kind == Some(FrameKind::Script);
        let name = if name_index == u16::MAX {
            None
        } else {
            Some(code.names.get(name_index as usize).ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {name_index} out of range (pool size {})",
                    code.names.len()
                ))
            })?)
        };

        // A materialized class namespace is the Python-visible source of truth.
        // Delete from it before clearing the implementation register, accepting
        // mapping-only bindings and rejecting stale register-only bindings.
        if frame_kind == Some(FrameKind::Class)
            && let (Some(name), Some(namespace)) = (name, active_live_class_namespace(self))
        {
            let Some(deleted) = namespace.dict_shift_remove(&PyKey::str_from(name))? else {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{name}' is not defined"),
                    Some(name.to_string()),
                ));
            };
            regs[register as usize] = Value::unset();
            call_del_if_last_binding(self, deleted, regs, code.num_locals as usize);
            return Ok(());
        }

        if let Some(name) = name
            && regs[register as usize].is_unset()
        {
            return if is_module_scope || frame_kind == Some(FrameKind::Class) {
                Err(PyError::name_error(
                    "NameError",
                    format!("name '{name}' is not defined"),
                    Some(name.to_string()),
                ))
            } else {
                Err(PyError::name_error(
                    "UnboundLocalError",
                    format!(
                        "cannot access local variable '{name}' where it is not associated with a value"
                    ),
                    None,
                ))
            };
        }

        let deleted = std::mem::replace(&mut regs[register as usize], Value::unset());
        if (!is_module_scope || !self.globals_exposed_for_environment(&self.env))
            && !deleted.is_unset()
        {
            call_del_if_last_binding(self, deleted, regs, code.num_locals as usize);
        }
        Ok(())
    }

    /// Mirror a module fast-local assignment into the live global namespace
    /// and maintain both global cache versions.
    #[inline]
    pub(crate) fn sync_module_global_binding(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        register: crate::bytecode::Reg,
        name_index: u16,
    ) -> Result<()> {
        let index = name_index as usize;
        let name_mask = code
            .global_cache_interest_masks
            .get(index)
            .copied()
            .ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {name_index} out of range (pool size {})",
                    code.names.len()
                ))
            })?;
        // Advance cache generations only when a published value or canonical
        // fallback cache registered a matching name mask. Ordinary scripts
        // then return `None` without materializing their lazy globals
        // dictionary.
        let (write_target, synchronize_siblings, synchronize_env) = {
            let environment = self.env.borrow();
            let target = environment.prepare_namespace_module_write(name_mask);
            let synchronize_siblings = !environment.namespace_cache_disabled()
                && environment.namespace_has_sibling_fastlocal_mirror();
            let synchronize_env = environment.namespace_env_binding_may_overlap(name_mask);
            (target, synchronize_siblings, synchronize_env)
        };
        if write_target.is_some() || synchronize_siblings || synchronize_env {
            let name = code.names.get(index).ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {name_index} out of range (pool size {})",
                    code.names.len()
                ))
            })?;
            let value = regs[register as usize].clone();
            if !value.is_unset() {
                if synchronize_env {
                    module_env(&self.env)
                        .borrow_mut()
                        .values
                        .insert(name, value.clone());
                }
                if let Some(target) = write_target {
                    let _ = target.dict_insert(PyKey::str_from(name), value.clone());
                }
                if synchronize_siblings {
                    self.env.borrow().synchronize_namespace_fastlocal_binding(
                        name,
                        &value,
                        Some(regs.non_null_ptr()),
                    );
                }
            }
        }
        Ok(())
    }

    /// Remove all module-namespace copies of a binding and run its finalizer
    /// only after the last Python-visible binding is gone.
    pub(crate) fn delete_module_global_binding(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        name_index: u16,
    ) -> Result<()> {
        let name = code.names.get(name_index as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: name index {name_index} out of range (pool size {})",
                code.names.len()
            ))
        })?;
        let from_env = module_env(&self.env).borrow_mut().values.remove(name);
        let locals = self
            .explicit_locals_for_environment(&self.env)
            .unwrap_or_else(|| self.active_globals_dict());
        let from_dict = locals
            .dict_shift_remove(&PyKey::str_from(name))
            .ok()
            .flatten();
        bump_global_struct_version(self);
        if !self.env.borrow().namespace_cache_disabled() {
            self.env
                .borrow()
                .synchronize_namespace_fastlocal_binding(name, &Value::unset(), None);
        }
        if let Some(value) = from_env.or(from_dict) {
            call_del_if_last_binding(self, value, regs, code.num_locals as usize);
        }
        Ok(())
    }

    /// Execute `from module import *`.
    ///
    /// Extracted from the `ImportStar` VM dispatch arm so that changes to
    /// star-import semantics (__all__ handling, filtering, etc.) only require
    /// touching this method rather than vm.rs.
    pub(crate) fn exec_import_star(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        mod_reg: crate::bytecode::Reg,
    ) -> Result<()> {
        let mod_val = vm_read(regs, mod_reg, num_locals)?;
        if !matches!(mod_val.kind(), ValueKind::PyModule(_)) {
            return Err(pyrust_core::type_err!(
                "import * requires a module, got {}",
                pyrust_core::builtin_type_name(&mod_val),
            ));
        }
        let ValueKind::PyModule(m) = mod_val.kind() else {
            unreachable!()
        };
        let pairs: Vec<(String, Value)> = {
            let (all_value, module_name) = {
                let borrowed = m.borrow();
                (borrowed.get_attr_value("__all__"), borrowed.name.clone())
            };
            if let Some(all_val) = all_value {
                // Own the names before dropping the module borrow. Lists are
                // RefCell-backed, so a borrowed view must not escape this
                // narrow read operation.
                let items = if let Some(items) = all_val.as_tuple() {
                    Some(items.to_vec())
                } else {
                    all_val.as_list().map(|items| items.to_vec())
                };
                let mut names: Vec<String> = Vec::new();
                let mut err: Option<PyError> = None;
                match items {
                    Some(items) => {
                        for item in &items {
                            match item.as_str() {
                                Some(s) => names.push(s.to_string()),
                                None => {
                                    err = Some(pyrust_core::type_err!(
                                        "Item in {}.__all__ must be str, not {}",
                                        module_name,
                                        pyrust_core::builtin_type_name(item),
                                    ));
                                    break;
                                }
                            }
                        }
                    }
                    None => {
                        err = Some(pyrust_core::type_err!(
                            "'{}' object does not support indexing",
                            pyrust_core::builtin_type_name(&all_val),
                        ));
                    }
                }
                if let Some(e) = err {
                    return Err(e);
                }
                let mut out = Vec::with_capacity(names.len());
                let mut attr_err: Option<(String, String)> = None;
                for name in &names {
                    match m.borrow().get_attr_value(name) {
                        Some(value) if !value.is_unset() => out.push((name.clone(), value)),
                        None => {
                            attr_err = Some((
                                format!("module '{}' has no attribute '{}'", module_name, name,),
                                name.clone(),
                            ));
                            break;
                        }
                        Some(_) => {
                            attr_err = Some((
                                format!("module '{}' has no attribute '{}'", module_name, name,),
                                name.clone(),
                            ));
                            break;
                        }
                    }
                }
                if let Some((attr_err_msg, attr_err_name)) = attr_err {
                    return Err(PyError::attribute_error(
                        attr_err_msg,
                        Some(attr_err_name),
                        None,
                    ));
                }
                out
            } else {
                m.borrow()
                    .attrs_snapshot()
                    .into_iter()
                    .filter(|(name, value)| !name.starts_with('_') && !value.is_unset())
                    .collect()
            }
        };
        for (name, val) in pairs {
            self.assign_name(&name, val);
        }
        Ok(())
    }

    /// Execute `del name` for module-scope and `global`-declared names.
    ///
    /// Extracted from the `DeleteName` VM dispatch arm so that changes to
    /// name-deletion semantics only require touching this method rather than
    /// vm.rs.
    pub(crate) fn exec_delete_name(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        name_idx: u16,
    ) -> Result<()> {
        let name = code.names.get(name_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: name index {} out of range (pool size {})",
                name_idx,
                code.names.len()
            ))
        })?;
        let is_global = self.env.borrow().global_names.contains(name.as_str());
        if is_global {
            let me = module_env(&self.env);
            let from_env = me.borrow_mut().values.remove(name.as_str());
            let in_env = from_env.is_some();
            let globals = self
                .explicit_globals_for_environment(&self.env)
                .unwrap_or_else(|| self.active_globals_dict());
            let from_dict = globals
                .dict_shift_remove(&PyKey::str_from(name.as_str()))
                .ok()
                .flatten();
            let in_dict = from_dict.is_some();
            if !in_env && !in_dict {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{}' is not defined", name),
                    Some(name.to_string()),
                ));
            }
            bump_global_struct_version(self);
            if !me.borrow().namespace_cache_disabled() {
                me.borrow().synchronize_namespace_fastlocal_binding(
                    name.as_str(),
                    &Value::unset(),
                    None,
                );
            }
            let del_candidate = from_env.or(from_dict);
            if let Some(val) = del_candidate {
                call_del_if_last_binding(self, val, regs, code.num_locals as usize);
            }
        } else {
            let from_env = self.env.borrow_mut().values.remove(name.as_str());
            let in_env = from_env.is_some();
            let is_module_scope = self.env.borrow().parent.is_none();
            let from_dict = if is_module_scope {
                self.explicit_globals_for_environment(&self.env)
                    .unwrap_or_else(|| self.active_globals_dict())
                    .dict_shift_remove(&PyKey::str_from(name.as_str()))
                    .ok()
                    .flatten()
            } else {
                None
            };
            let in_dict = from_dict.is_some();
            if !in_env && !in_dict {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{}' is not defined", name),
                    Some(name.to_string()),
                ));
            }
            if is_module_scope {
                bump_global_struct_version(self);
                if !self.env.borrow().namespace_cache_disabled() {
                    self.env.borrow().synchronize_namespace_fastlocal_binding(
                        name.as_str(),
                        &Value::unset(),
                        None,
                    );
                }
            }
            let del_candidate = from_env.or(from_dict);
            if let Some(val) = del_candidate {
                call_del_if_last_binding(self, val, regs, code.num_locals as usize);
            }
        }
        Ok(())
    }
}
