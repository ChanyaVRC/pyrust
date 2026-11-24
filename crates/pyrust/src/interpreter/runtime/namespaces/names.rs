// Lexical and module name mutation.
impl Interpreter {
    /// Return `true` when the currently active frame explicitly declared `name`
    /// as `global` (including functions whose defining `env` is a non-root class
    /// or PEP 695 type-parameter scope, without allocating a per-call
    /// `Environment`, issue #452).
    ///
    /// Skips the active function's `global_names` while a temporary
    /// `PushTypeParamEnv` child scope is active (`!Rc::ptr_eq(&self.env, &func.env)`),
    /// so binding a PEP 695 type parameter `T` inside a function that also has
    /// `global T` still writes the `TypeVar` into the type-param scope.
    #[inline]
    pub(crate) fn active_frame_declares_global(&self, name: &str) -> bool {
        if let Some(view) = self.vm_frame_views.last() {
            if let Some(func) = view.function() {
                if Rc::ptr_eq(&self.env, &func.env) && func.global_names.contains(name) {
                    return true;
                }
            } else if let Some(gen_frame) = view.gen_frame() {
                // SAFETY: `VmFrameView::gen_frame` points to a heap-stable
                // `GeneratorFrame` for as long as the view is on the stack.
                let gf = unsafe { gen_frame.as_ref() };
                if Rc::ptr_eq(&self.env, &gf.function.env)
                    && gf.function.global_names.contains(name)
                {
                    return true;
                }
            }
        }
        self.env.borrow().global_names.contains(name)
    }

    pub(crate) fn assign_name(&self, name: &str, value: Value) {
        let is_global = self.active_frame_declares_global(name);
        if is_global {
            // Write to the module env HashMap so LoadGlobal / post-run
            // inspection can find the new value.
            let root = module_env(&self.env);
            {
                let root_ref = root.borrow();
                if root_ref.namespace_has_binding_order() {
                    root_ref.record_namespace_binding_order_insert(name);
                } else if !root_ref.namespace_cache_disabled()
                    && root_ref.namespace_has_active_fastlocal_mirror()
                    && root_ref.namespace_values().get(name).is_none()
                    && root_ref.namespace_fastlocal_value(name).is_none()
                {
                    root_ref.record_namespace_out_of_order_insert(name);
                }
            }
            {
                let mut root = root.borrow_mut();
                root.record_namespace_env_binding(name);
                root.namespace_values_mut().insert(name, value.clone());
            }
            // Invalidate the LoadGlobal inline cache: any function that cached
            // this global under the current version will re-fetch on its next call.
            bump_global_struct_version(self);
            // Mirror into the live module globals dict only when globals() has
            // been called.
            if let Some(globals) = self.globals_write_target_for_environment(&self.env) {
                let _ = globals.dict_insert(PyKey::str_from(name), value.clone());
            }
            // Before the namespace mapping escapes there is no storage-level
            // callback, so propagate a function's global write to every live
            // Script mirror owned by the root. Once exposed, dict insertion
            // above performs the same update at the storage boundary.
            if !root.borrow().namespace_cache_disabled() {
                root.borrow()
                    .synchronize_namespace_fastlocal_binding(name, &value, None);
            }
            return;
        }
        // Module scope: `self.env` is the root env (no parent).  Mirror into
        // the root-owned globals backing only after it has been exposed.
        let is_module_scope = self.env.borrow().parent.is_none();
        if is_module_scope {
            {
                let env_ref = self.env.borrow();
                if env_ref.namespace_has_binding_order() {
                    env_ref.record_namespace_binding_order_insert(name);
                } else if !env_ref.namespace_cache_disabled()
                    && env_ref.namespace_has_active_fastlocal_mirror()
                    && env_ref.namespace_values().get(name).is_none()
                    && !env_ref.namespace_fastlocal_declares(name)
                {
                    env_ref.record_namespace_out_of_order_insert(name);
                }
            }
            // Invalidate the LoadGlobal inline cache for module-scope writes.
            bump_global_struct_version(self);
            if let Some(globals) = self.globals_write_target_for_environment(&self.env) {
                let _ = globals.dict_insert(PyKey::str_from(name), value.clone());
            }
            if !self.env.borrow().namespace_cache_disabled() {
                self.env
                    .borrow()
                    .synchronize_namespace_fastlocal_binding(name, &value, None);
            }
        }
        env_assign_local(&self.env, name, value);
    }

    /// Resolve `name` against the active scope, reporting an unbound binding as a
    /// plain local (`UnboundLocalError`).  Used by the interpreter unit tests'
    /// `lookup_name` assertions; the VM read opcodes call
    /// [`lookup_name_inner`](Self::lookup_name_inner) directly so they can flag a
    /// captured free variable (issue #2340).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn lookup_name(&self, name: &str) -> Result<Option<Value>> {
        self.lookup_name_inner(name, false)
    }

    /// Resolve the active module's display name for runtime objects created by
    /// language syntax. The lexical environment is authoritative; the live
    /// globals dict is the fallback for user mutation through `globals()`,
    /// followed by the configured builtins provider for explicit exec roots.
    pub(crate) fn defining_module_name(&self) -> Result<String> {
        let namespace = self.prepare_global_namespace("__name__");
        let resolved = self.lookup_name_inner("__name__", false)?.or_else(|| {
            namespace
                .globals()
                .dict_with(|dict| dict.get(&StrKey("__name__")).cloned())
                .flatten()
        });
        if let Some(name) = resolved.and_then(|value| value.as_str().map(str::to_string)) {
            return Ok(name);
        }
        builtins_provider_module_name(namespace.globals()).ok_or_else(|| undefined_name("__name__"))
    }

    /// Name resolution for `Insn::LoadGlobal` against the active namespace chain
    /// (module, class, or PEP 695 type-parameter scope). Function-scope cells
    /// and `nonlocal` bindings bypass this path entirely via indexed
    /// `LoadCell` / `StoreCell` / `DeleteCell` (issues #452 / #2348).
    pub(crate) fn lookup_name_inner(&self, name: &str, is_free: bool) -> Result<Option<Value>> {
        if self.active_frame_declares_global(name) {
            return Ok(lookup_name_in_module(&self.env, name));
        }
        if is_free {
            return lookup_name_in_env_as_free(&self.env, name);
        }
        lookup_name_in_env(&self.env, name)
    }

    pub(super) fn alloc_env(&mut self, parent: Option<EnvRef>) -> EnvRef {
        if let Some(env) = self.cold.env_pool.pop() {
            {
                let mut e = env.borrow_mut();
                e.reset_for_reuse(parent);
            }
            env
        } else {
            Environment::new(parent)
        }
    }

    pub(super) fn free_env(&mut self, env: EnvRef) {
        if self.cold.env_pool.len() < ENV_POOL_MAX && Rc::strong_count(&env) == 1 {
            self.cold.env_pool.push(env);
        }
    }
}

/// Read the configured builtins provider's module name without invoking user
/// attribute code. Native modules synthesize `__name__` from their internal
/// name; explicit dict and module namespaces must carry their own value.
fn builtins_provider_module_name(globals: &Value) -> Option<String> {
    let provider = globals
        .dict_with(|dict| dict.get(&StrKey("__builtins__")).cloned())
        .flatten()
        .unwrap_or_else(cached_builtins_module);
    match provider.kind() {
        ValueKind::Dict(_) => provider
            .dict_with(|dict| dict.get(&StrKey("__name__")).cloned())
            .flatten()
            .and_then(|value| value.as_str().map(str::to_string)),
        ValueKind::PyModule(module) => {
            let module = module.borrow();
            match module.get_attr_value("__name__") {
                Some(value) if !value.is_unset() => value.as_str().map(str::to_string),
                Some(_) => None,
                None if module.live_namespace().is_none()
                    && module.filesystem_namespace().is_none() =>
                {
                    Some(module.name.clone())
                }
                None => None,
            }
        }
        _ => None,
    }
}
