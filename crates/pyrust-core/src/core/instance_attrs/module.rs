/// Live namespace linkage used only by source-backed filesystem modules.
///
/// `globals` is the exact dictionary also retained by the root environment and
/// returned by functions' `__globals__`. The environment link is deliberately
/// weak: circular imports may place module objects in each other's namespaces,
/// so a strong link here would add an uncollectable
/// `PyModule -> Environment -> PyModule` ownership cycle. A captured function
/// keeps its own environment alive; a module containing only data remains
/// usable from the strongly-owned globals dictionary after the loader
/// interpreter and environment have gone away. As with an ordinary exposed
/// `globals()` dictionary, storing a function back into its own globals can
/// still form a Python-level dict/function/environment cycle; collecting that
/// general object-graph cycle is outside this module state and is not made
/// worse by a second strong module-to-environment edge.
#[derive(Debug, Clone)]
pub struct FilesystemModuleNamespace {
    environment: Weak<RefCell<Environment>>,
    globals: Value,
}

impl FilesystemModuleNamespace {
    fn new(environment: &EnvRef, globals: Value) -> Self {
        debug_assert!(globals.is_dict());
        Self {
            environment: Rc::downgrade(environment),
            globals,
        }
    }

    #[inline]
    pub fn environment(&self) -> Option<EnvRef> {
        self.environment.upgrade()
    }

    #[inline]
    pub fn globals(&self) -> Value {
        self.globals.clone()
    }
}

/// Receives one startup namespace slot during [`LazyModuleAttrSource::visit_slots`]
/// as `(name, deferred)`.
///
/// `deferred == true` names a slot the module resolves on demand; `false`
/// marks a slot whose value was stored in the namespace when the module was
/// built.
pub type LazyModuleSlotVisitor<'a> = &'a mut dyn FnMut(&'static str, bool);

/// Builds the value of one seeded deferred attribute (see
/// [`PyModule::set_deferred_attr`]).
pub type LazyModuleAttrInit = Box<dyn FnOnce() -> Value>;

/// Static description of the attributes a native module resolves on demand.
///
/// Both callbacks must be pure functions of the name.  A read never stores
/// the resolved value in the namespace, so no version bump is needed and
/// opcode caches keyed on [`ModuleMutationState`] stay sound.
#[derive(Debug, Clone, Copy)]
pub struct LazyModuleAttrSource {
    /// Resolve one deferred attribute.  Must return `None` — without building
    /// anything — for every name this source does not own.
    pub resolve: fn(&str) -> Option<Value>,
    /// Walk the module's startup namespace in insertion order, flagging every
    /// deferred slot.
    pub visit_slots: fn(LazyModuleSlotVisitor<'_>),
    /// Who owns a resolved value.
    ///
    /// `false`: `resolve` hands back the same per-GIL-group object every time
    /// (the `builtins` classes), so the module remembers nothing.
    ///
    /// `true`: `resolve` builds an object that belongs to this module object
    /// (`sys.monitoring`, issue #3293).  The module remembers the first one in
    /// its pending state — still outside the namespace — so repeated reads
    /// observe one identity and two interpreters never share it.
    pub per_module: bool,
}

/// One value owned by a module's pending deferred state.
enum LazyOwnedSlot {
    /// Already resolved; every later read returns this object.
    Ready(Value),
    /// Seeded by the embedding runtime, not yet built.
    Seeded(LazyModuleAttrInit),
}

/// Pending deferred attributes of one module object.
///
/// The state is all-or-nothing: either every deferred slot is still pending
/// (and absent from the namespace), or the whole set has been materialized and
/// this state dropped.  Rebinding or deleting a pending name materializes
/// first, so a deferred value can never overwrite a user binding or reappear
/// after `del`.
struct LazyModuleAttrs {
    source: LazyModuleAttrSource,
    /// Number of leading namespace entries that still sit at their startup
    /// position.  A startup name that was deleted and re-bound lives past this
    /// boundary, so materialization appends it instead of restoring its
    /// original slot — exactly what a Python `dict` would have done.
    head_len: usize,
    /// Values this module object owns while they are still deferred: resolved
    /// `per_module` slots and seeded initializers.  Empty (and never
    /// allocated) for a `per_module == false` source.
    owned: RefCell<Vec<(Box<str>, LazyOwnedSlot)>>,
}

impl std::fmt::Debug for LazyModuleAttrs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyModuleAttrs")
            .field("source", &self.source)
            .field("head_len", &self.head_len)
            .field("owned", &self.owned.borrow().len())
            .finish()
    }
}

impl LazyModuleAttrs {
    /// Resolve one deferred attribute without touching the namespace.
    fn resolve(&self, name: &str) -> Option<Value> {
        if !self.source.per_module {
            return (self.source.resolve)(name);
        }
        let seeded = {
            let mut owned = self.owned.borrow_mut();
            match owned.iter().position(|(key, _)| &**key == name) {
                Some(index) => match &owned[index].1 {
                    LazyOwnedSlot::Ready(value) => return Some(value.clone()),
                    LazyOwnedSlot::Seeded(_) => Some(owned.swap_remove(index).1),
                },
                None => None,
            }
        };
        // Built with the borrow released: an initializer is free to read
        // other deferred slots of the same module.
        let value = match seeded {
            Some(LazyOwnedSlot::Seeded(init)) => init(),
            Some(LazyOwnedSlot::Ready(value)) => value,
            None => (self.source.resolve)(name)?,
        };
        self.owned
            .borrow_mut()
            .push((name.into(), LazyOwnedSlot::Ready(value.clone())));
        Some(value)
    }

    /// Whether `name` is one of this module's deferred slots.
    ///
    /// Never builds a module-owned value just to answer: a `per_module`
    /// source is asked through its slot table instead of its resolver.
    fn owns(&self, name: &str) -> bool {
        if !self.source.per_module {
            return (self.source.resolve)(name).is_some();
        }
        let mut declared = false;
        (self.source.visit_slots)(&mut |slot, deferred| {
            declared |= deferred && slot == name;
        });
        declared
    }

    /// `current` with every deferred slot resolved and placed at its startup
    /// position.
    fn merged_namespace(&self, current: &ModuleAttrs) -> ModuleAttrs {
        let head_len = self.head_len.min(current.len());
        let mut placed = vec![false; current.len()];
        let mut merged = ModuleAttrs::default();
        (self.source.visit_slots)(&mut |name, deferred| {
            if deferred {
                if let Some(value) = self.resolve(name) {
                    merged.entry(name.to_string()).or_insert(value);
                }
            } else if let Some((index, key, value)) = current.get_full(name)
                && index < head_len
            {
                placed[index] = true;
                merged.insert(key.clone(), value.clone());
            }
        });
        // Everything bound after startup (and any startup name that was
        // deleted and re-bound) follows in its own insertion order.
        for (index, (key, value)) in current.iter().enumerate() {
            if !placed[index] {
                merged.insert(key.clone(), value.clone());
            }
        }
        merged
    }
}

#[derive(Debug)]
pub struct PyModule {
    pub name: String,
    /// Insertion-ordered so `vars(module)` matches the module's declaration
    /// order and is identical run to run (issue #2918).
    pub attrs: ModuleAttrs,
    mutation_state: ModuleMutationState,
    /// Exact Python dictionary exposed by built-in and `types.ModuleType`
    /// modules once their namespace is accessed via `module.__dict__` /
    /// `vars(module)` (or eagerly for `sys` and `types.ModuleType` instances,
    /// issue #2970).
    ///
    /// Built-in modules that never expose `__dict__` retain the compact direct
    /// `attrs` fast path and keep equality-based caches enabled.  First access
    /// to `__dict__` lazily upgrades to this live dictionary and saturates
    /// `mutation_state`, so direct dictionary writes and attribute access share
    /// one authoritative backing without slowing down hot unexposed modules.
    live_namespace: Option<Value>,
    /// Boxed so the built-in-module common case pays only one nullable pointer
    /// and continues to use its existing direct `attrs` fast path.
    filesystem_namespace: Option<Box<FilesystemModuleNamespace>>,
    /// Attributes resolved on demand instead of stored in the namespace (the
    /// `builtins` class objects, issue #3294; the heavy `sys` attributes,
    /// issue #3293).  Boxed so every other module pays one nullable pointer.
    /// Honoured by compact `attrs` storage and by a live namespace that has
    /// not been handed to Python code yet; never set on a filesystem module.
    lazy_attrs: Option<Box<LazyModuleAttrs>>,
    /// Set while the embedding runtime still owes this module an
    /// initialization step that must run before Python code observes the
    /// affected attributes (the `zipimport` path hook of `sys`, issue #3293).
    /// The runtime owns the meaning; this is only the one-byte marker it
    /// tests on its attribute slow path.
    deferred_init: Cell<bool>,
}

impl PyModule {
    pub fn new(name: String, attrs: ModuleAttrs) -> Self {
        Self {
            name,
            attrs,
            mutation_state: ModuleMutationState::fresh(),
            live_namespace: None,
            filesystem_namespace: None,
            lazy_attrs: None,
            deferred_init: Cell::new(false),
        }
    }

    /// Build a module whose `attrs` hold only the eagerly stored slots of
    /// `source`; the deferred ones stay unallocated until first use.
    pub fn with_lazy_attrs(name: String, attrs: ModuleAttrs, source: LazyModuleAttrSource) -> Self {
        let head_len = attrs.len();
        let mut module = Self::new(name, attrs);
        module.lazy_attrs = Some(Box::new(LazyModuleAttrs {
            source,
            head_len,
            owned: RefCell::new(Vec::new()),
        }));
        module
    }

    /// Whether the embedding runtime still owes this module a deferred
    /// initialization step.
    #[inline]
    pub fn has_deferred_init(&self) -> bool {
        self.deferred_init.get()
    }

    /// Mark or clear the deferred initialization step.
    #[inline]
    pub fn set_deferred_init(&self, pending: bool) {
        self.deferred_init.set(pending);
    }

    /// Bind `name` to the value `init` builds, without building it while the
    /// name is still one of this module's deferred slots.
    ///
    /// This is how a per-interpreter value (`sys.flags`) joins a module-owned
    /// deferred source: the slot keeps its startup position and stays
    /// unallocated until first use.  Once the deferred set has been
    /// materialized this is an ordinary rebind of the freshly built value.
    pub fn set_deferred_attr(&mut self, name: &str, init: LazyModuleAttrInit) {
        if let Some(lazy) = self.lazy_attrs.as_deref()
            && lazy.source.per_module
            && !self.namespace_contains(name)
            && lazy.owns(name)
        {
            let mut owned = lazy.owned.borrow_mut();
            owned.retain(|(key, _)| &**key != name);
            owned.push((name.into(), LazyOwnedSlot::Seeded(init)));
            return;
        }
        self.insert_attr(name.to_string(), init());
    }

    /// Whether `name` is still an unresolved deferred slot: nothing has been
    /// built for it on behalf of this module object.
    pub fn deferred_attr_is_unresolved(&self, name: &str) -> bool {
        let Some(lazy) = self.lazy_attrs.as_deref() else {
            return false;
        };
        if self.namespace_contains(name) {
            return false;
        }
        !lazy
            .owned
            .borrow()
            .iter()
            .any(|(key, slot)| &**key == name && matches!(slot, LazyOwnedSlot::Ready(_)))
    }

    /// Whether the authoritative namespace binds `name` (deferred slots are
    /// not bound until materialized).
    fn namespace_contains(&self, name: &str) -> bool {
        match self.live_namespace.as_ref() {
            Some(namespace) => namespace
                .dict_with(|dict| dict.contains_key(&crate::object_model::StrKey(name)))
                .unwrap_or(false),
            None => self.attrs.contains_key(name),
        }
    }

    /// String-keyed view of a live namespace, in dictionary order.
    fn live_namespace_attrs(namespace: &Value) -> ModuleAttrs {
        namespace
            .dict_with(|dict| {
                dict.iter()
                    .filter_map(|(key, value)| {
                        let PyKey::Str(key) = key else {
                            return None;
                        };
                        key.as_str().map(|name| (name.to_string(), value.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether this module still has deferred attributes that were never
    /// materialized into its namespace.
    #[inline]
    pub fn has_pending_lazy_attrs(&self) -> bool {
        self.lazy_attrs.is_some()
    }

    /// Resolve every deferred attribute into the namespace, in startup order.
    ///
    /// Name-to-value bindings are unchanged, but entry positions move, so the
    /// provider version is bumped for any cache that remembers a slot index.
    pub fn materialize_lazy_attrs(&mut self) {
        let Some(lazy) = self.lazy_attrs.take() else {
            return;
        };
        match self.live_namespace.as_ref() {
            Some(namespace) => {
                // The live dict keeps its identity; only its entries are
                // re-laid in startup order.  Non-string keys cannot exist yet
                // (the dict is materialized before Python code can alias it),
                // but are carried over rather than dropped.
                let current = Self::live_namespace_attrs(namespace);
                let mut entries: Vec<(PyKey, Value)> = lazy
                    .merged_namespace(&current)
                    .into_iter()
                    .map(|(name, value)| (PyKey::str_from(&name), value))
                    .collect();
                if let Some(others) = namespace.dict_with(|dict| {
                    dict.iter()
                        .filter(|(key, _)| !matches!(key, PyKey::Str(_)))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>()
                }) {
                    entries.extend(others);
                }
                namespace
                    .dict_clear()
                    .expect("live module namespace must remain a dict");
                namespace
                    .dict_extend(entries)
                    .expect("live module namespace must remain a dict");
            }
            None => self.attrs = lazy.merged_namespace(&self.attrs),
        }
        self.mutation_state.bump();
    }

    /// Materialize before a write that targets a still-deferred name, so the
    /// write then behaves exactly as it would on an eagerly built namespace.
    #[inline]
    fn materialize_lazy_attrs_for_write(&mut self, name: &str) {
        if let Some(lazy) = self.lazy_attrs.as_deref()
            && !self.namespace_contains(name)
            && lazy.owns(name)
        {
            self.materialize_lazy_attrs();
        }
    }

    /// Move this native module's attributes into one stable Python dict.
    ///
    /// Calling this before publishing the module makes later attribute writes
    /// and direct `module.__dict__` mutations share the same backing identity.
    ///
    /// Deferred attributes stay pending: the dictionary is not visible to
    /// Python code until [`Self::exposed_live_namespace`] (or
    /// [`Self::attach_live_namespace_with_dunders`]) hands it out, and both
    /// materialize first.
    pub fn attach_live_namespace(&mut self) {
        debug_assert!(self.filesystem_namespace.is_none());
        if self.live_namespace.is_some() {
            return;
        }
        let attrs = std::mem::take(&mut self.attrs)
            .into_iter()
            .map(|(name, value)| (PyKey::str_from(&name), value))
            .collect();
        self.live_namespace = Some(Value::dict(attrs));
        self.mutation_state.bump();
    }

    /// The live namespace dictionary, for identity and mutation-state checks.
    ///
    /// While deferred attributes are pending the dictionary does not list
    /// them, so a caller that hands the dictionary to Python code must use
    /// [`Self::exposed_live_namespace`] instead.
    #[inline]
    pub fn live_namespace(&self) -> Option<Value> {
        self.live_namespace.clone()
    }

    /// The live namespace dictionary as Python code may observe it: every
    /// deferred attribute is materialized into it first.
    pub fn exposed_live_namespace(&mut self) -> Option<Value> {
        self.live_namespace.as_ref()?;
        self.materialize_lazy_attrs();
        self.live_namespace.clone()
    }

    /// Lazily upgrade a built-in module from compact `attrs` storage into one
    /// stable, live Python dictionary exposed as `module.__dict__` /
    /// `vars(module)` (issue #2970).
    ///
    /// Seeds CPython's five module-object dunder slots (`__name__`, `__doc__`,
    /// `__package__`, `__loader__`, `__spec__`) at the head in CPython's order,
    /// omitting any slot previously tombstoned via `del`, then extends with all
    /// live attributes in declaration order so rebound slots keep their head
    /// position (issue #2918 / PR #2969).  Permanently disables equality-based
    /// caches on `mutation_state` because arbitrary Python aliases to the
    /// returned dict can now mutate the namespace directly.
    pub fn attach_live_namespace_with_dunders(&mut self) -> Value {
        debug_assert!(self.filesystem_namespace.is_none());
        self.materialize_lazy_attrs();
        if let Some(namespace) = self.live_namespace.as_ref() {
            self.mutation_state.disable_cache();
            return namespace.clone();
        }
        let existing = std::mem::take(&mut self.attrs);
        let is_live = |key: &str| !existing.get(key).is_some_and(Value::is_unset);
        let mut dict: PyDict =
            PyDict::with_capacity_and_hasher(existing.len() + 5, Default::default());
        if is_live("__name__") {
            dict.insert(PyKey::str_from("__name__"), Value::string(&self.name));
        }
        if is_live("__doc__") {
            dict.insert(PyKey::str_from("__doc__"), Value::none());
        }
        if is_live("__package__") {
            dict.insert(PyKey::str_from("__package__"), Value::string(String::new()));
        }
        if is_live("__loader__") {
            dict.insert(PyKey::str_from("__loader__"), Value::none());
        }
        if is_live("__spec__") {
            dict.insert(PyKey::str_from("__spec__"), Value::none());
        }
        dict.extend(
            existing
                .into_iter()
                .filter(|(_, value)| !value.is_unset())
                .map(|(name, value)| (PyKey::str_from(&name), value)),
        );
        let namespace = Value::dict(dict);
        self.live_namespace = Some(namespace.clone());
        self.mutation_state.disable_cache();
        namespace
    }

    /// Permanently disable equality-based caches for this module provider when
    /// a live `__dict__` alias is handed to Python code.
    #[inline]
    pub fn disable_cache(&self) {
        self.mutation_state.disable_cache();
    }

    /// Format `repr(module)`, mirroring CPython's `module_repr` (`<module %R>`
    /// from `__name__`, or `<module '?'>` when `__name__` is absent/deleted,
    /// falling back to the built-in module's initial `name` only while its
    /// namespace has not been materialized and `__name__` has not been
    /// modified or tombstoned).
    pub fn repr_string(&self) -> String {
        let has_explicit_namespace =
            self.live_namespace.is_some() || self.filesystem_namespace.is_some();
        match self.get_attr_value("__name__") {
            Some(v) if !v.is_unset() => format!("<module {}>", v.repr_raw()),
            Some(_) => "<module '?'>".to_string(),
            None if has_explicit_namespace => "<module '?'>".to_string(),
            None => format!("<module {}>", Value::string(&self.name).repr_raw()),
        }
    }

    /// Attach this source-backed module to the environment executing its body.
    ///
    /// The root's pre-existing globals `Value` becomes the module's sole live
    /// Python dictionary. No attribute snapshot is harvested after execution.
    pub fn attach_filesystem_namespace(&mut self, environment: &EnvRef) {
        let globals = environment.borrow().namespace_globals();
        environment
            .borrow()
            .configure_filesystem_module_namespace(self.mutation_state());
        self.lazy_attrs = None;
        self.attrs.clear();
        self.filesystem_namespace = Some(Box::new(FilesystemModuleNamespace::new(
            environment,
            globals,
        )));
    }

    /// Attach the top-level `__main__` module to the script's root environment
    /// without forcing eager globals mirroring (#2957).
    ///
    /// Unlike an imported filesystem module (which runs under `SharedGlobals`),
    /// `__main__` runs with `FastLocals` registers. Keeping eager mirroring off
    /// preserves full register speed when `__main__` is never inspected, while
    /// first observation of `__main__` attributes or `__dict__` flushes and
    /// exposes the exact same live globals dict (`sys.modules['__main__'].__dict__
    /// is globals()`).
    pub fn attach_main_namespace(&mut self, environment: &EnvRef) {
        let globals = environment.borrow().namespace_globals();
        self.lazy_attrs = None;
        self.attrs.clear();
        self.filesystem_namespace = Some(Box::new(FilesystemModuleNamespace::new(
            environment,
            globals,
        )));
    }

    #[inline]
    pub fn filesystem_namespace(&self) -> Option<FilesystemModuleNamespace> {
        self.filesystem_namespace.as_deref().cloned()
    }

    /// Read one attribute from the module's authoritative namespace.
    #[inline]
    pub fn get_attr_value(&self, name: &str) -> Option<Value> {
        if let Some(namespace) = self.live_namespace.as_ref() {
            let bound = namespace
                .dict_with(|dict| dict.get(&crate::object_model::StrKey(name)).cloned())
                .flatten();
            if bound.is_some() {
                return bound;
            }
            return self
                .lazy_attrs
                .as_deref()
                .and_then(|lazy| lazy.resolve(name));
        }
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            return namespace
                .globals
                .dict_with(|dict| dict.get(&crate::object_model::StrKey(name)).cloned())
                .flatten();
        }
        if let Some(value) = self.attrs.get(name) {
            return Some(value.clone());
        }
        self.lazy_attrs
            .as_deref()
            .and_then(|lazy| lazy.resolve(name))
    }

    /// Snapshot string-keyed attributes for generic reflection paths.
    ///
    /// Filesystem module dictionaries may contain non-string keys through a
    /// direct `__dict__` alias; those are visible in the dictionary itself but
    /// are not Python attributes and are intentionally omitted here.
    pub fn attrs_snapshot(&self) -> ModuleAttrs {
        if let Some(namespace) = self.live_namespace.as_ref() {
            let current = Self::live_namespace_attrs(namespace);
            return match self.lazy_attrs.as_deref() {
                Some(lazy) => lazy.merged_namespace(&current),
                None => current,
            };
        }
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            return namespace
                .globals
                .dict_with(|dict| {
                    dict.iter()
                        .filter_map(|(key, value)| {
                            let PyKey::Str(key) = key else {
                                return None;
                            };
                            key.as_str().map(|name| (name.to_string(), value.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        match self.lazy_attrs.as_deref() {
            Some(lazy) => lazy.merged_namespace(&self.attrs),
            None => self.attrs.clone(),
        }
    }

    /// Collect the attributes currently bound in the namespace that `keep`
    /// accepts, in namespace order.
    ///
    /// Unlike [`Self::attrs_snapshot`] this clones only the accepted entries
    /// and never resolves a deferred slot, so a caller scanning a freshly
    /// imported module for a few entries of interest neither copies the whole
    /// namespace nor forces the deferred attributes into existence.
    pub fn collect_bound_attrs(
        &self,
        mut keep: impl FnMut(&str, &Value) -> bool,
    ) -> Vec<(String, Value)> {
        let dictionary = self
            .live_namespace
            .as_ref()
            .or(self.filesystem_namespace.as_deref().map(|ns| &ns.globals));
        if let Some(dictionary) = dictionary {
            return dictionary
                .dict_with(|dict| {
                    dict.iter()
                        .filter_map(|(key, value)| {
                            let PyKey::Str(key) = key else {
                                return None;
                            };
                            let name = key.as_str()?;
                            keep(name, value).then(|| (name.to_string(), value.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        self.attrs
            .iter()
            .filter(|(name, value)| keep(name, value))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    /// Return the stable provider token used by module-backed namespace caches.
    #[inline]
    pub fn mutation_state(&self) -> ModuleMutationState {
        self.mutation_state.clone()
    }

    /// Insert or replace one module attribute and invalidate namespace caches.
    #[inline]
    pub fn insert_attr(&mut self, name: String, value: Value) -> Option<Value> {
        if self.live_namespace.is_some() {
            self.materialize_lazy_attrs_for_write(&name);
            let namespace = self.live_namespace.as_ref().expect("checked above");
            let previous = namespace
                .dict_insert(crate::object_model::PyKey::str_from(&name), value)
                .expect("live module namespace must remain a dict");
            self.mutation_state.bump();
            return previous;
        }
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            let previous = namespace
                .globals
                .dict_insert(crate::object_model::PyKey::str_from(&name), value.clone())
                .expect("filesystem module globals must remain a dict");
            if let Some(environment) = namespace.environment() {
                let mut environment = environment.borrow_mut();
                environment.record_namespace_env_binding(&name);
                environment
                    .namespace_values_mut()
                    .insert(&name, value.clone());
                environment.synchronize_namespace_fastlocal_binding(&name, &value, None);
                environment.bump_namespace_structure_version();
            }
            self.mutation_state.bump();
            return previous;
        }
        self.materialize_lazy_attrs_for_write(&name);
        let previous = self.attrs.insert(name, value);
        self.mutation_state.bump();
        previous
    }

    /// Remove one module attribute and invalidate namespace caches when found.
    #[inline]
    pub fn remove_attr(&mut self, name: &str) -> Option<Value> {
        if self.live_namespace.is_some() {
            self.materialize_lazy_attrs_for_write(name);
            let namespace = self.live_namespace.as_ref().expect("checked above");
            let index = match self.lazy_attrs.as_deref() {
                Some(_) => namespace
                    .dict_with(|dict| dict.get_index_of(&crate::object_model::StrKey(name)))
                    .flatten(),
                None => None,
            };
            let removed = namespace
                .dict_shift_remove(&crate::object_model::PyKey::str_from(name))
                .expect("live module namespace must remain a dict");
            if removed.is_some() {
                if let Some(index) = index
                    && let Some(lazy) = self.lazy_attrs.as_deref_mut()
                    && index < lazy.head_len
                {
                    lazy.head_len -= 1;
                }
                self.mutation_state.bump();
            }
            return removed;
        }
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            let removed = namespace
                .globals
                .dict_shift_remove(&crate::object_model::PyKey::str_from(name))
                .expect("filesystem module globals must remain a dict");
            if removed.is_some() {
                if let Some(environment) = namespace.environment() {
                    let mut environment = environment.borrow_mut();
                    environment.namespace_values_mut().remove(name);
                    environment.synchronize_namespace_fastlocal_binding(
                        name,
                        &Value::unset(),
                        None,
                    );
                    environment.bump_namespace_structure_version();
                }
                self.mutation_state.bump();
            }
            return removed;
        }
        // `shift_remove`, not `swap_remove`: deleting one attribute must not
        // reorder the surviving ones, matching `del d[k]` on a Python dict and
        // the `dict_shift_remove` used by both namespace-backed branches above.
        self.materialize_lazy_attrs_for_write(name);
        let (index, _, removed) = self.attrs.shift_remove_full(name)?;
        if let Some(lazy) = self.lazy_attrs.as_deref_mut()
            && index < lazy.head_len
        {
            lazy.head_len -= 1;
        }
        self.mutation_state.bump();
        Some(removed)
    }

    /// Replace the complete module namespace while preserving its storage kind.
    #[inline]
    pub fn replace_attrs(&mut self, attrs: ModuleAttrs) {
        if let Some(namespace) = self.live_namespace.as_ref() {
            self.lazy_attrs = None;
            namespace
                .dict_clear()
                .expect("live module namespace must remain a dict");
            namespace
                .dict_extend(
                    attrs
                        .into_iter()
                        .map(|(name, value)| (PyKey::str_from(&name), value))
                        .collect(),
                )
                .expect("live module namespace must remain a dict");
            self.mutation_state.bump();
            return;
        }
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            namespace
                .globals
                .dict_clear()
                .expect("filesystem module globals must remain a dict");
            if let Some(environment) = namespace.environment() {
                environment.borrow_mut().namespace_values_mut().clear();
            }
            for (name, value) in attrs {
                let _ = namespace
                    .globals
                    .dict_insert(crate::object_model::PyKey::str_from(&name), value.clone());
                if let Some(environment) = namespace.environment() {
                    let mut environment = environment.borrow_mut();
                    environment.record_namespace_env_binding(&name);
                    environment.namespace_values_mut().insert(&name, value);
                }
            }
            if let Some(environment) = namespace.environment() {
                environment
                    .borrow()
                    .synchronize_namespace_fastlocals_from_mapping(&namespace.globals);
            }
            self.mutation_state.bump();
            return;
        }
        self.lazy_attrs = None;
        self.attrs = attrs;
        self.mutation_state.bump();
    }
}

impl Clone for PyModule {
    fn clone(&self) -> Self {
        // Cloning the struct creates a new module object, not a Python alias
        // (aliases clone the surrounding `Value`/`Rc`).  Give that object a
        // distinct provider identity rather than sharing cache generations.
        Self::new(self.name.clone(), self.attrs_snapshot())
    }
}
