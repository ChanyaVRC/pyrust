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
                } else if !root_ref.namespace_mapping_authoritative()
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
            self.mirror_global_write(name, &value);
            // A normal root propagates a function's global write to every
            // live Script mirror it owns itself: the dictionary write above is
            // an owner write, which the storage notification skips. Only an
            // explicit provider relies on that notification.
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
                } else if !env_ref.namespace_mapping_authoritative()
                    && env_ref.namespace_has_active_fastlocal_mirror()
                    && env_ref.namespace_values().get(name).is_none()
                    && !env_ref.namespace_fastlocal_declares(name)
                {
                    env_ref.record_namespace_out_of_order_insert(name);
                }
            }
            // Invalidate the LoadGlobal inline cache for module-scope writes.
            bump_global_struct_version(self);
            self.mirror_global_write(name, &value);
            if !self.env.borrow().namespace_cache_disabled() {
                self.env
                    .borrow()
                    .synchronize_namespace_fastlocal_binding(name, &value, None);
            }
        }
        env_assign_local(&self.env, name, value);
    }

    /// Mirror one global binding this interpreter has just written into the
    /// dictionary that must stay live for it, if any.
    ///
    /// The write is an owner write (see
    /// [`Environment::write_namespace_mapping_binding`]): the caller has
    /// already advanced the cache generations and updates the root's Script
    /// mirrors itself, so an exposed normal namespace is not invalidated a
    /// second time at the dictionary-storage boundary.
    #[inline]
    fn mirror_global_write(&self, name: &str, value: &Value) {
        let Some(globals) = self.globals_write_target_for_environment(&self.env) else {
            return;
        };
        let displaced =
            self.env
                .borrow()
                .write_namespace_mapping_binding(&globals, name, value.clone());
        drop(displaced);
    }

    /// Repeat a completed [`Self::assign_name`] as an in-place overwrite of the
    /// root binding `slot` located (#3423).
    ///
    /// Returns `None`, having changed nothing, when the overwrite cannot be
    /// proven equivalent to the full assignment; the caller then runs
    /// [`Self::assign_name`]. On success the caller passes the outcome to
    /// [`Self::finish_cached_global_store`] once it holds no cache borrow.
    ///
    /// A frame whose lexical environment is the root writes the same root
    /// binding whether or not it declares the name `global`, so no declaration
    /// check is needed there. Any other frame reaches the root only through a
    /// `global` declaration, which `declared_in` proves.
    #[inline]
    pub(crate) fn overwrite_cached_global_slot(
        &self,
        name: &str,
        name_mask: u64,
        slot: &pyrust_core::NamespaceStoreSlot,
        declared_in: Option<&pyrust_core::NameSet>,
        value: &Value,
    ) -> Option<pyrust_core::NamespaceStoreOutcome> {
        let mut env = self.env.borrow_mut();
        if env.parent.is_none() {
            return env.overwrite_namespace_store_slot(slot, name, name_mask, value);
        }
        drop(env);
        self.overwrite_declared_global_slot(name, name_mask, slot, declared_in?, value)
    }

    /// Non-root half of [`Self::overwrite_cached_global_slot`]: a function
    /// defined inside a class body or a PEP 695 scope that declares the name
    /// `global`.
    ///
    /// `declared_in` is the `global`-name set that contained `name` when the
    /// cache entry was filled. The entry keeps that set alive, so finding the
    /// same allocation on the active function proves the declaration without
    /// hashing the name again. A temporary PEP 695 child scope
    /// (`self.env != function.env`) declines, as in
    /// [`Self::active_frame_declares_global`].
    #[inline(never)]
    fn overwrite_declared_global_slot(
        &self,
        name: &str,
        name_mask: u64,
        slot: &pyrust_core::NamespaceStoreSlot,
        declared_in: &pyrust_core::NameSet,
        value: &Value,
    ) -> Option<pyrust_core::NamespaceStoreOutcome> {
        let function = self.vm_frame_views.last()?.function()?;
        if !Rc::ptr_eq(&self.env, &function.env) || !Rc::ptr_eq(&function.global_names, declared_in)
        {
            return None;
        }
        module_env(&self.env)
            .borrow_mut()
            .overwrite_namespace_store_slot(slot, name, name_mask, value)
    }

    /// Complete an in-place global overwrite after every borrow of the root
    /// environment and of the bytecode cache has been released: mirror the
    /// write into the globals dictionary that must stay live (a filesystem
    /// module's shared backing or an exposed mapping), then drop the displaced
    /// values.
    #[inline]
    pub(crate) fn finish_cached_global_store(
        &self,
        name: &str,
        value: Value,
        outcome: pyrust_core::NamespaceStoreOutcome,
    ) {
        if let Some(globals) = &outcome.globals {
            // An owner write: the in-place overwrite already updated the root
            // binding and its Script register, and it advances no generation,
            // so the storage notification must not invalidate this root.
            let displaced = self
                .env
                .borrow()
                .write_namespace_mapping_binding(globals, name, value);
            drop(displaced);
        }
    }

    /// Decide whether the [`Self::assign_name`] that just completed for `name`
    /// can be repeated as an in-place overwrite, and build its cache locator
    /// (#3423).
    ///
    /// Only writes that reach the root namespace are cacheable: every store
    /// from a frame whose lexical environment is the root, and a `global`
    /// store from a function defined in a nested scope (for which the active
    /// function's `global`-name set is returned as the declaration proof).
    /// A class-body or type-parameter binding, and a generator frame nested in
    /// one, stay on the full path.
    pub(crate) fn global_store_cache_slot(
        &self,
        name: &str,
    ) -> Option<(
        pyrust_core::NamespaceStoreSlot,
        Option<pyrust_core::NameSet>,
    )> {
        {
            let env = self.env.borrow();
            if env.parent.is_none() {
                return env.namespace_store_slot(name).map(|slot| (slot, None));
            }
        }
        let function = self.vm_frame_views.last()?.function()?;
        if !Rc::ptr_eq(&self.env, &function.env) || !function.global_names.contains(name) {
            return None;
        }
        let declared_in = Rc::clone(&function.global_names);
        module_env(&self.env)
            .borrow()
            .namespace_store_slot(name)
            .map(|slot| (slot, Some(declared_in)))
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
