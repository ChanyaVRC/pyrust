/// A single lexical scope's binding storage (issues #452 / #3162).
///
/// Each `Environment` stores a name-keyed [`EnvValues`] namespace (module,
/// class body, PEP 695 type-parameter scope, or explicit `exec`/`eval`
/// namespace) via [`EnvironmentStorage`]. Ordinary function calls do not
/// allocate an `Environment` at all: their locals live in frame registers,
/// their captured cells live in the frame's `CellSlot` slice /
/// `UserFunction::closure_cells`, and their `env` points straight at the
/// enclosing module/class/type-param namespace.
///
/// The authoritative description of namespace lookup rules lives in
/// `crates/pyrust/src/interpreter/helpers/name_resolution.rs` (search for
/// "the env-lookup rule (issue #452)"). Keep these in sync.
#[derive(Debug, Clone)]
enum ClassAnnotationProvider {
    Active(EnvValues),
    Mapping(Value),
    Owner(Rc<RefCell<PyClass>>),
}

#[derive(Debug, Clone)]
pub struct Environment {
    /// Identity, mutation generations, and globals provider shared by every
    /// lexical scope below the same root namespace.
    root_namespace: Rc<RootNamespaceState>,
    /// Name-keyed binding storage (issues #452 / #3162).
    pub storage: EnvironmentStorage,
    pub local_names: NameSet,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub parent: Option<EnvRef>,
    /// PEP 695 class-body annotation scopes resolve ordinary names through the
    /// class namespace, unlike methods defined in the same body. While the
    /// body is executing this is a register snapshot kept current by
    /// `RecordClassStore`/`RecordClassDel`; after class construction it is
    /// replaced by a weak reference to the live class namespace.
    ///
    /// Keeping this provider on the dedicated annotation environment (rather
    /// than the shared class environment) prevents bare names in methods from
    /// incorrectly seeing class attributes.
    class_annotation: Option<Box<ClassAnnotationProvider>>,
}

pub type EnvRef = Rc<RefCell<Environment>>;

impl Environment {
    #[inline]
    fn root_namespace_for_parent(parent: Option<&EnvRef>) -> Rc<RootNamespaceState> {
        parent
            .map(|parent| Rc::clone(&parent.borrow().root_namespace))
            .unwrap_or_else(|| Rc::new(RootNamespaceState::new()))
    }

    pub fn new(parent: Option<EnvRef>) -> EnvRef {
        let root_namespace = Self::root_namespace_for_parent(parent.as_ref());
        Rc::new(RefCell::new(Self {
            root_namespace,
            storage: EnvironmentStorage::Namespace(EnvValues::new()),
            local_names: Rc::new(HashSet::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            parent,
            class_annotation: None,
        }))
    }

    /// Borrow the name-keyed namespace bindings.
    #[inline]
    pub fn namespace_values(&self) -> &EnvValues {
        match &self.storage {
            EnvironmentStorage::Namespace(values) => values,
        }
    }

    /// Mutably borrow the name-keyed namespace bindings.
    #[inline]
    pub fn namespace_values_mut(&mut self) -> &mut EnvValues {
        match &mut self.storage {
            EnvironmentStorage::Namespace(values) => values,
        }
    }

    /// Reset a uniquely-owned environment before returning it from the runtime
    /// pool.  Inheriting the parent's shared state is essential: a pooled frame
    /// may previously have belonged to a different module namespace.
    pub fn reset_for_reuse(&mut self, parent: Option<EnvRef>) {
        self.root_namespace = Self::root_namespace_for_parent(parent.as_ref());
        self.namespace_values_mut().clear();
        self.parent = parent;
        self.local_names = Rc::new(HashSet::new());
        self.global_names = Rc::new(HashSet::new());
        self.nonlocal_names = Rc::new(HashSet::new());
        self.class_annotation = None;
    }

    /// Mark this lexical environment as a PEP 695 class annotation scope.
    pub fn initialize_class_annotation_scope(&mut self) {
        self.class_annotation = Some(Box::new(ClassAnnotationProvider::Active(EnvValues::new())));
    }

    /// Mirror a class-body fast-local store while the class is still running.
    pub fn set_class_annotation_binding(&mut self, name: &str, value: Value) {
        let provider = self
            .class_annotation
            .get_or_insert_with(|| Box::new(ClassAnnotationProvider::Active(EnvValues::new())));
        if let ClassAnnotationProvider::Active(values) = provider.as_mut() {
            values.insert(name, value);
        }
    }

    /// Mirror a class-body fast-local deletion while the class is still running.
    pub fn remove_class_annotation_binding(&mut self, name: &str) {
        if let Some(ClassAnnotationProvider::Active(values)) = self.class_annotation.as_deref_mut()
        {
            values.remove(name);
        }
    }

    /// Use the live plain-dict namespace prepared by a metaclass while its
    /// constructor is running. Generic custom mappings remain on the mirrored
    /// register provider because consulting `__getitem__` requires interpreter
    /// dispatch and cannot live in the core lexical environment.
    pub fn bind_class_annotation_mapping(&mut self, namespace: Value) {
        self.class_annotation = Some(Box::new(ClassAnnotationProvider::Mapping(namespace)));
    }

    /// Switch from the temporary class-body provider to the finished class's
    /// live namespace. This is a semantic ownership edge: an alias extracted
    /// from a short-lived class must still be able to evaluate against that
    /// class namespace later.
    pub fn bind_class_annotation_owner(&mut self, class: &Rc<RefCell<PyClass>>) {
        self.class_annotation = Some(Box::new(ClassAnnotationProvider::Owner(Rc::clone(class))));
    }

    /// Resolve one name from this environment's class-annotation provider.
    ///
    /// The caller checks this provider before the environment's type-parameter
    /// bindings. CPython's class annotation bytecode uses
    /// `LOAD_FROM_DICT_OR_DEREF`: a same-named class attribute wins, with the
    /// type parameter used only when the class namespace has no such key.
    #[inline]
    pub fn class_annotation_binding(&self, name: &str) -> Option<Value> {
        match self.class_annotation.as_deref()? {
            ClassAnnotationProvider::Owner(owner) => owner.borrow().attrs.get(name).cloned(),
            ClassAnnotationProvider::Mapping(namespace) => namespace
                .dict_with(|dict| dict.get(&StrKey(name)).cloned())
                .flatten(),
            ClassAnnotationProvider::Active(values) => values.get(name).cloned(),
        }
    }

    /// Stable identity of the root namespace. `u64::MAX` is an uncacheable
    /// saturation sentinel.
    #[inline]
    pub fn namespace_id(&self) -> u64 {
        self.root_namespace.id
    }

    /// Return the complete inline-cache key state with one environment borrow.
    #[inline]
    pub fn namespace_cache_snapshot(&self) -> (u64, u64, u64, u64, bool) {
        (
            self.root_namespace.id,
            self.root_namespace.environment_version.get(),
            self.root_namespace.structure_version.get(),
            self.root_namespace.fastlocal_mirror_epoch.get(),
            self.root_namespace.cache_disabled.get(),
        )
    }

    #[inline]
    pub fn bump_namespace_environment_version(&self) {
        self.root_namespace.bump_environment_version();
    }

    /// Register a precomputed name mask for an environment-backed value that
    /// is about to be published in a `LoadGlobal` cache.
    #[inline]
    pub fn register_namespace_value_cache_interest(&self, name_mask: u64) {
        self.root_namespace
            .value_cache_interest
            .set(self.root_namespace.value_cache_interest.get() | name_mask);
        self.root_namespace.recompute_sync_fast_check_mask();
    }

    /// Register a precomputed name mask for a canonical fallback value that is
    /// about to be published in a `LoadGlobal` cache.
    #[inline]
    pub fn register_namespace_fallback_cache_interest(&self, name_mask: u64) {
        self.root_namespace
            .fallback_cache_interest
            .set(self.root_namespace.fallback_cache_interest.get() | name_mask);
        self.root_namespace.recompute_sync_fast_check_mask();
    }

    #[inline]
    pub fn record_namespace_env_binding(&self, name: &str) {
        self.root_namespace.env_binding_interest.set(
            self.root_namespace.env_binding_interest.get() | namespace_name_interest_mask(name),
        );
        self.root_namespace.recompute_sync_fast_check_mask();
    }

    #[inline]
    pub fn namespace_env_binding_may_overlap(&self, name_mask: u64) -> bool {
        self.root_namespace.env_binding_interest.get() & name_mask == name_mask
    }

    /// Single-bitmask pre-check for `Insn::SyncModuleGlobal` (#2924).
    ///
    /// Returns `false` only when no cache interest mask or slow env binding
    /// overlaps `name_mask`, no globals/module dictionary needs mirroring, and
    /// no sibling fast-local mirror is active.
    #[inline(always)]
    pub fn needs_namespace_module_write_sync(&self, name_mask: u64) -> bool {
        (self.root_namespace.sync_fast_check_mask.get() & name_mask) == name_mask
    }
}
