// globals/locals namespace operations.
impl Interpreter {
    /// Delete a cell-slot binding in O(1) (`Insn::DeleteCell`, issues #452 / #2348).
    ///
    /// When `raise_if_missing` is true (source `del x`), an already-empty slot
    /// raises `UnboundLocalError` (for an own cell) or free-variable `NameError`
    /// (for a `nonlocal` / freevar cell). When false (PEP 3110 `except E as e:`
    /// cleanup), clearing an empty slot is an idempotent no-op.
    pub(crate) fn exec_delete_cell(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        cells: CellSlice,
        cell_idx: u16,
        raise_if_missing: bool,
    ) -> Result<()> {
        let deleted = cells.as_slice()[cell_idx as usize].take();
        let Some(deleted) = deleted else {
            if raise_if_missing {
                let meta = code.cell_slots[cell_idx as usize];
                return Err(unbound_binding_error(
                    meta.name.as_str(),
                    meta.is_free_error,
                ));
            }
            return Ok(());
        };
        call_del_if_last_binding_in_env(
            self,
            deleted,
            &self.env.clone(),
            regs,
            code.num_locals as usize,
        );
        Ok(())
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
            .is_some_and(|view| view.kind() == FrameKind::Function);
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
        raise_if_missing: bool,
    ) -> Result<()> {
        let frame_kind = self.vm_frame_views.last().map(|view| view.kind());
        let is_module_scope = frame_kind == Some(FrameKind::Script);
        let name = code.names.get(name_index as usize).ok_or_else(|| {
            PyError::runtime(format!(
                "bytecode error: name index {name_index} out of range (pool size {})",
                code.names.len()
            ))
        })?;

        // A materialized class namespace is the Python-visible source of truth.
        // Delete from it before clearing the implementation register. A source
        // `del` rejects a missing key; compiler-generated except cleanup is
        // idempotent because nested handlers may reuse and clear the same slot.
        if frame_kind == Some(FrameKind::Class)
            && let Some(namespace) = active_live_class_namespace(self)
        {
            if let Some(deleted) = namespace.dict_shift_remove(&PyKey::str_from(name))? {
                regs[register as usize] = Value::unset();
                call_del_if_last_binding(self, deleted, regs, code.num_locals as usize);
                return Ok(());
            }
            if raise_if_missing {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{name}' is not defined"),
                    Some(name.to_string()),
                ));
            }
            let deleted = std::mem::replace(&mut regs[register as usize], Value::unset());
            if !deleted.is_unset() {
                call_del_if_last_binding(self, deleted, regs, code.num_locals as usize);
            }
            return Ok(());
        }

        if raise_if_missing && regs[register as usize].is_unset() {
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

        if is_module_scope && !regs[register as usize].is_unset() {
            self.env
                .borrow()
                .record_namespace_binding_order_delete(name);
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
    #[inline(always)]
    pub(crate) fn sync_module_global_binding(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        register: crate::bytecode::Reg,
        name_index: u16,
    ) -> Result<()> {
        let index = name_index as usize;
        if let Some(&name_mask) = code.global_cache_interest_masks.get(index)
            && !self
                .env
                .borrow()
                .needs_namespace_module_write_sync(name_mask)
        {
            return Ok(());
        }
        self.sync_module_global_binding_dispatch_slow(code, regs, register, name_index)
    }

    #[cold]
    #[inline(never)]
    fn sync_module_global_binding_dispatch_slow(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        register: crate::bytecode::Reg,
        name_index: u16,
    ) -> Result<()> {
        let index = name_index as usize;
        let Some(&name_mask) = code.global_cache_interest_masks.get(index) else {
            return Self::invalid_name_index_error(name_index, code.names.len());
        };
        // Advance cache generations only when a published value or canonical
        // fallback cache registered a matching name mask. Ordinary scripts
        // return `0` without materializing their lazy globals dictionary or
        // touching the out-of-line slow path.
        let sync_flags = self
            .env
            .borrow()
            .prepare_namespace_module_write_flags(name_mask);
        if sync_flags != 0 {
            return self
                .sync_module_global_binding_slow(code, regs, register, name_index, sync_flags);
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn invalid_name_index_error(name_index: u16, pool_len: usize) -> Result<()> {
        Err(PyError::runtime(format!(
            "bytecode error: name index {name_index} out of range (pool size {pool_len})"
        )))
    }

    /// Out-of-line slow path for [`Self::sync_module_global_binding`].
    ///
    /// Keeping dict/env/mirror/order synchronization out of the `#[inline]` VM
    /// dispatch arm prevents hot-loop code-size and register-pressure
    /// regressions while using `EnvValues::update_if_present` so a
    /// `env_binding_interest` Bloom false positive never inserts an absent key
    /// or perturbs namespace ordering (issues #2919, #2925).
    #[cold]
    #[inline(never)]
    fn sync_module_global_binding_slow(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        register: crate::bytecode::Reg,
        name_index: u16,
        sync_flags: u8,
    ) -> Result<()> {
        let Some(name) = code.names.get(name_index as usize) else {
            return Self::invalid_name_index_error(name_index, code.names.len());
        };
        let value = regs[register as usize].clone();
        if value.is_unset() {
            return Ok(());
        }
        if sync_flags & Environment::MODULE_WRITE_TRACK_BINDING_ORDER != 0 {
            self.env
                .borrow()
                .record_namespace_binding_order_insert(name);
        }
        let write_target = if sync_flags & Environment::MODULE_WRITE_SYNC_GLOBALS != 0 {
            self.env.borrow().namespace_module_write_target()
        } else {
            None
        };
        let synchronize_siblings = sync_flags & Environment::MODULE_WRITE_SYNC_SIBLINGS != 0;
        let synchronize_env = sync_flags & Environment::MODULE_WRITE_SYNC_ENV != 0;
        if synchronize_env {
            let env_value = if write_target.is_none() && !synchronize_siblings {
                module_env(&self.env)
                    .borrow_mut()
                    .namespace_values_mut()
                    .update_if_present(name, value);
                return Ok(());
            } else {
                value.clone()
            };
            module_env(&self.env)
                .borrow_mut()
                .namespace_values_mut()
                .update_if_present(name, env_value);
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
            PyError::runtime(format!(
                "bytecode error: name index {name_index} out of range (pool size {})",
                code.names.len()
            ))
        })?;
        let root = module_env(&self.env);
        {
            let root_ref = root.borrow();
            if root_ref.namespace_values().get(name).is_some()
                || root_ref.namespace_has_binding_order()
            {
                root_ref.record_namespace_binding_order_delete(name);
            }
        }
        let from_env = root.borrow_mut().namespace_values_mut().remove(name);
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
                let is_package = self.module_has_package_path(&mod_val)?;
                let mut out = Vec::with_capacity(names.len());
                let mut attr_err: Option<(String, String)> = None;
                for name in &names {
                    let existing = m.borrow().get_attr_value(name);
                    match existing {
                        Some(value) if !value.is_unset() => out.push((name.clone(), value)),
                        _ => {
                            if is_package {
                                let sub_name = format!("{module_name}.{name}");
                                match self.load_module(&sub_name) {
                                    Ok(sub_val) => {
                                        out.push((name.clone(), sub_val));
                                        continue;
                                    }
                                    Err(load_err)
                                        if self.is_missing_module_error(&load_err, &sub_name) => {}
                                    Err(load_err) => return Err(load_err),
                                }
                            }
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
            PyError::runtime(format!(
                "bytecode error: name index {} out of range (pool size {})",
                name_idx,
                code.names.len()
            ))
        })?;
        let is_global = self.active_frame_declares_global(name.as_str());
        if is_global {
            let me = module_env(&self.env);
            let (in_env, from_mirror) = {
                let me_ref = me.borrow();
                (
                    me_ref.namespace_values().get(name.as_str()).is_some(),
                    me_ref.namespace_fastlocal_value(name.as_str()),
                )
            };
            let globals = self
                .explicit_globals_for_environment(&self.env)
                .unwrap_or_else(|| self.active_globals_dict());
            let in_dict = globals
                .dict_with(|dict| dict.contains_key(&StrKey(name.as_str())))
                .unwrap_or(false);
            if !in_env && !in_dict && from_mirror.is_none() {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{}' is not defined", name),
                    Some(name.to_string()),
                ));
            }
            me.borrow()
                .record_namespace_binding_order_delete(name.as_str());
            let from_env = me.borrow_mut().namespace_values_mut().remove(name.as_str());
            let from_dict = globals
                .dict_shift_remove(&PyKey::str_from(name.as_str()))
                .ok()
                .flatten();
            bump_global_struct_version(self);
            if !me.borrow().namespace_cache_disabled() {
                me.borrow().synchronize_namespace_fastlocal_binding(
                    name.as_str(),
                    &Value::unset(),
                    None,
                );
            }
            let del_candidate = from_env.or(from_dict).or(from_mirror);
            if let Some(val) = del_candidate {
                call_del_if_last_binding(self, val, regs, code.num_locals as usize);
            }
        } else {
            let (is_module_scope, in_env, from_mirror) = {
                let env_ref = self.env.borrow();
                let is_module_scope = env_ref.parent.is_none();
                (
                    is_module_scope,
                    env_ref
                        .try_namespace_values()
                        .is_some_and(|values| values.get(name.as_str()).is_some()),
                    if is_module_scope {
                        env_ref.namespace_fastlocal_value(name.as_str())
                    } else {
                        None
                    },
                )
            };
            let globals = is_module_scope.then(|| {
                self.explicit_globals_for_environment(&self.env)
                    .unwrap_or_else(|| self.active_globals_dict())
            });
            let in_dict = globals.as_ref().is_some_and(|globals| {
                globals
                    .dict_with(|dict| dict.contains_key(&StrKey(name.as_str())))
                    .unwrap_or(false)
            });
            if !in_env && !in_dict && from_mirror.is_none() {
                return Err(PyError::name_error(
                    "NameError",
                    format!("name '{}' is not defined", name),
                    Some(name.to_string()),
                ));
            }
            if is_module_scope {
                self.env
                    .borrow()
                    .record_namespace_binding_order_delete(name.as_str());
            }
            let from_env = self
                .env
                .borrow_mut()
                .try_namespace_values_mut()
                .and_then(|values| values.remove(name.as_str()));
            let from_dict = globals.and_then(|globals| {
                globals
                    .dict_shift_remove(&PyKey::str_from(name.as_str()))
                    .ok()
                    .flatten()
            });
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
            let del_candidate = from_env.or(from_dict).or(from_mirror);
            if let Some(val) = del_candidate {
                call_del_if_last_binding(self, val, regs, code.num_locals as usize);
            }
        }
        Ok(())
    }
}
