/// Shared identity and mutation generation for one concrete module namespace.
///
/// `PyModule` itself lives behind `Rc<RefCell<_>>`, but inline caches must be
/// able to observe namespace mutations without retaining (and potentially
/// cycling through) the module object.  Cloning this token is cheap, preserves
/// provider identity, and shares the generation counter across every
/// interpreter that references the same module. `u64::MAX` is a saturated
/// sentinel at which equality-based caches remain permanently disabled.
#[derive(Debug, Clone)]
pub struct ModuleMutationState(Rc<Cell<u64>>);

impl ModuleMutationState {
    fn fresh() -> Self {
        Self(Rc::new(Cell::new(0)))
    }

    #[inline]
    pub fn version(&self) -> u64 {
        self.0.get()
    }

    /// Return the version only while module-backed caches may safely use it.
    #[inline]
    pub fn cache_version(&self) -> Option<u64> {
        let version = self.version();
        (version != u64::MAX).then_some(version)
    }

    /// Validate a cached version without accepting the saturated sentinel.
    #[inline]
    pub fn matches_cache_version(&self, cached: u64) -> bool {
        cached != u64::MAX && self.version() == cached
    }

    #[inline]
    pub fn same_provider(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    #[inline]
    pub(crate) fn bump(&self) {
        self.0.set(self.0.get().saturating_add(1));
    }

    /// Permanently disable equality-based caches for this module provider.
    ///
    /// A filesystem module's live `__dict__` can be mutated through arbitrary
    /// aliases that do not pass through `PyModule`. Saturation makes those
    /// mutations safe without adding a watcher to every ordinary dictionary.
    #[inline]
    pub(crate) fn disable_cache(&self) {
        self.0.set(u64::MAX);
    }
}
