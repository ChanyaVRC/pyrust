// Lexical and module name mutation.
impl Interpreter {
    pub(crate) fn assign_name(&self, name: &str, value: Value) {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (
                env.global_names.contains(name),
                env.nonlocal_names.contains(name),
            )
        };
        if is_global {
            // Write to the module env HashMap so LoadGlobal / post-run
            // inspection can find the new value.
            let root = module_env(&self.env);
            {
                let mut root = root.borrow_mut();
                root.record_namespace_env_binding(name);
                root.values.insert(name, value.clone());
            }
            // Invalidate the LoadGlobal inline cache: any function that cached
            // this global under the current version will re-fetch on its next call.
            bump_global_struct_version(self);
            // Mirror into the live module globals dict only when globals() has
            // been called. Without this guard,
            // every StoreGlobal pays an extra IndexMap write even for scripts
            // that never use globals() — the primary cause of the ~15x
            // regression introduced by PR #810. globals() marks the root and
            // does a one-time sync before returning, so subsequent writes keep
            // the dict live from that point on.
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
        if is_nonlocal && let Some(env) = find_enclosing_local_env_for_name(&self.env, name) {
            env_assign_local(&env, name, value);
            return;
        }
        // Module scope: `self.env` is the root env (no parent).  Mirror into
        // the root-owned globals backing only after it has been exposed.
        let is_module_scope = self.env.borrow().parent.is_none();
        if is_module_scope {
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
    /// globals dict is the fallback for user mutation through `globals()`.
    pub(crate) fn defining_module_name(&self) -> Result<String> {
        let _namespace = self.prepare_global_namespace("__name__");
        self.lookup_name_inner("__name__", false).map(|resolved| {
            resolved
                .or_else(|| {
                    self.active_globals_dict()
                        .dict_with(|dict| dict.get(&StrKey("__name__")).cloned())
                        .flatten()
                })
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "__main__".to_string())
        })
    }

    /// Name resolution for `Insn::LoadGlobal` / `Insn::LoadCell`.
    ///
    /// `is_free` selects the CPython 3.12 error class for an unbound binding
    /// found via the plain env walk (issue #2340): when set, the name is a
    /// captured **free** variable of the currently-executing function (not one
    /// of its own locals / cell vars), so an unbound binding is reported as a
    /// `NameError` ("cannot access free variable ... in enclosing scope")
    /// rather than an `UnboundLocalError`.  A `nonlocal` read always binds to an
    /// enclosing scope and so is treated as free regardless of `is_free`.
    pub(crate) fn lookup_name_inner(&self, name: &str, is_free: bool) -> Result<Option<Value>> {
        let (is_global, is_nonlocal) = {
            let env = self.env.borrow();
            (
                env.global_names.contains(name),
                env.nonlocal_names.contains(name),
            )
        };
        if is_global {
            return Ok(lookup_name_in_module(&self.env, name));
        }
        if is_nonlocal {
            return lookup_name_in_enclosing_local_env(&self.env, name);
        }
        if is_free {
            return lookup_name_in_env_as_free(&self.env, name);
        }
        lookup_name_in_env(&self.env, name)
    }

    pub(super) fn alloc_env(&mut self, parent: Option<EnvRef>) -> EnvRef {
        if let Some(env) = self.env_pool.pop() {
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
        if self.env_pool.len() < ENV_POOL_MAX && Rc::strong_count(&env) == 1 {
            self.env_pool.push(env);
        }
    }
}
