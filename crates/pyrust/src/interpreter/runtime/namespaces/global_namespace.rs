pub(crate) struct GlobalNamespace {
    root: EnvRef,
    globals: Value,
    lookup_locals: Option<Value>,
    explicit: bool,
    sync_backing: bool,
}

impl GlobalNamespace {
    #[inline]
    pub(crate) fn globals(&self) -> &Value {
        &self.globals
    }

    #[inline]
    pub(crate) fn is_explicit(&self) -> bool {
        self.explicit
    }

    #[inline]
    pub(crate) fn cacheable(&self) -> bool {
        !self.root.borrow().namespace_cache_disabled()
    }

    /// Refresh one root binding from the live explicit namespace before the
    /// existing lexical lookup walks into that root.  Only the requested name
    /// is synchronized, so a mutation does not copy the whole globals dict.
    fn sync_name(&self, name: &str) {
        if !self.sync_backing {
            return;
        }
        let from_locals = self.lookup_locals.as_ref().and_then(|locals| {
            locals
                .dict_with(|dict| dict.get(&StrKey(name)).cloned())
                .flatten()
        });
        let value = from_locals.or_else(|| {
            self.globals
                .dict_with(|dict| dict.get(&StrKey(name)).cloned())
                .flatten()
        });
        let mut root = self.root.borrow_mut();
        if let Some(value) = value {
            root.record_namespace_env_binding(name);
            root.values.insert(name, value);
        } else {
            root.values.remove(name);
        }
    }
}

impl Interpreter {
    /// Associate a fresh root environment with caller-owned globals/locals.
    /// The provider lives in the root's shared namespace state, so functions
    /// retain it without an interpreter-local registry.
    pub(super) fn register_explicit_global_namespace(
        &self,
        root: &EnvRef,
        globals: Value,
        locals: Option<Value>,
    ) {
        root.borrow().configure_explicit_namespace(globals, locals);
    }

    /// Resolve the root namespace backing the currently active lexical frame.
    pub(crate) fn prepare_global_namespace(&self, name: &str) -> GlobalNamespace {
        let root = module_env(&self.env);
        let (globals, locals, is_explicit, cache_disabled) = {
            let root = root.borrow();
            let (globals, locals, explicit) = root.namespace_globals_provider();
            (globals, locals, explicit, root.namespace_cache_disabled())
        };
        let at_explicit_script_scope = is_explicit
            && self
                .vm_frame_views
                .last()
                .is_some_and(|view| view.kind == FrameKind::Script);
        let namespace = GlobalNamespace {
            root,
            globals,
            lookup_locals: at_explicit_script_scope.then_some(locals).flatten(),
            explicit: is_explicit,
            // Explicit dicts and exposed normal globals are authoritative and
            // may be mutated without an interpreter callback.
            sync_backing: cache_disabled,
        };
        namespace.sync_name(name);
        namespace
    }

    /// Return the caller-owned globals dict attached to an explicit root.
    pub(crate) fn explicit_globals_for_environment(&self, env: &EnvRef) -> Option<Value> {
        let root = module_env(env);
        root.borrow().namespace_explicit_globals()
    }

    /// Return the Python locals mapping attached to an explicit root.  When
    /// exec/eval received no separate locals argument, globals is also locals.
    pub(crate) fn explicit_locals_for_environment(&self, env: &EnvRef) -> Option<Value> {
        let root = module_env(env);
        root.borrow().namespace_explicit_locals()
    }

    /// Root-owned globals backing for internal runtime synchronization. This
    /// does not expose the mapping or disable caches.
    pub(crate) fn namespace_globals_for_environment(&self, env: &EnvRef) -> Value {
        module_env(env).borrow().namespace_globals()
    }

    /// Expose the live globals mapping for an arbitrary captured environment.
    ///
    /// Python-visible access through function.__globals__, frame.f_globals, or
    /// generator/traceback frames has the same alias-mutation surface as
    /// globals(). Normal roots are synchronized and permanently uncached.
    /// Explicit exec/eval globals are already authoritative and must not be
    /// overwritten from their captured EnvValues snapshot.
    pub(crate) fn globals_for_environment(&self, env: &EnvRef) -> Value {
        let root = module_env(env);
        if let Some(globals) = root.borrow().namespace_explicit_globals() {
            return globals;
        }

        let already_exposed = root.borrow().namespace_globals_exposed();
        let globals = root.borrow().expose_namespace_globals();
        if already_exposed {
            // After first exposure the mapping is authoritative: copying the
            // EnvValues snapshot again could overwrite an alias mutation that
            // has not yet been read back through LoadGlobal.
            return globals;
        }
        // Binding order, not map order: the dictionary this produces is a
        // Python dict and CPython guarantees module-namespace insertion order
        // (issue #2903).
        let pairs = root.borrow().namespace_materialization_snapshot();
        for (name, value) in pairs {
            let _ = globals.dict_insert(PyKey::str_from(&name), value);
        }
        root.borrow().activate_namespace_globals_alias(&globals);
        globals
    }

    /// Expose the live `f_locals` mapping of a module-scope frame (issue #2926).
    ///
    /// CPython does not snapshot a module frame's locals: `f_locals` *is* the
    /// module dictionary, so `frame.f_locals is frame.f_globals is globals()`
    /// and a write through it is a global binding. Under `exec(code, g, l)` the
    /// module code's locals are the caller's `l` instead, which is exactly what
    /// the explicit provider records.
    ///
    /// Routing through the same provider `globals()` uses is what makes the
    /// identity hold; building a dict here would reintroduce the copy.
    pub(crate) fn frame_locals_for_module_environment(&self, env: &EnvRef) -> Value {
        self.explicit_locals_for_environment(env)
            .unwrap_or_else(|| self.globals_for_environment(env))
    }

    /// Whether controlled module writes must mirror into the globals backing.
    ///
    /// Ordinary scripts require mirroring only after globals/locals escape.
    /// Filesystem modules require it from attachment time because `PyModule`
    /// attributes use this same dictionary even while opcode caches remain
    /// enabled.
    pub(crate) fn globals_exposed_for_environment(&self, env: &EnvRef) -> bool {
        module_env(env)
            .borrow()
            .namespace_globals_require_mirroring()
    }

    /// Mapping that must be kept live for a namespace mutation. Explicit
    /// globals are always authoritative; filesystem-module globals are always
    /// mirrored into their shared `PyModule` backing; other normal globals are
    /// mirrored only after Python code obtains an alias.
    pub(crate) fn globals_write_target_for_environment(&self, env: &EnvRef) -> Option<Value> {
        let root = module_env(env);
        let root = root.borrow();
        if let Some(globals) = root.namespace_explicit_globals() {
            Some(globals)
        } else if root.namespace_globals_require_mirroring() {
            Some(root.namespace_globals())
        } else {
            None
        }
    }

    /// Internal backing for the active root namespace. This does not expose the
    /// mapping; Python-visible callers must use `globals_for_environment`.
    pub(crate) fn active_globals_dict(&self) -> Value {
        self.namespace_globals_for_environment(&self.env)
    }

    /// Python-visible locals for the active explicit script scope.
    pub(crate) fn active_locals_dict(&self) -> Value {
        self.explicit_locals_for_environment(&self.env)
            .unwrap_or_else(|| self.active_globals_dict())
    }
}
