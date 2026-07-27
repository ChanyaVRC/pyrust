thread_local! {
    /// Stable identities for root/global namespaces on this interpreter
    /// thread. `u64::MAX` is a saturated, uncacheable sentinel.
    static NEXT_ROOT_NAMESPACE_ID: Cell<u64> = const { Cell::new(1) };
    /// Weak owners of Python-visible namespace dictionaries, keyed by the
    /// identity of their shared `Rc<RefCell<PyDict>>` backing.
    static NAMESPACE_ALIAS_OWNERS: RefCell<HashMap<usize, Vec<Weak<RootNamespaceState>>>> =
        RefCell::new(HashMap::new());
    /// Ordinary dictionaries pay only this branch until a namespace mapping
    /// has actually escaped on the current interpreter thread.
    static HAS_NAMESPACE_ALIASES: Cell<bool> = const { Cell::new(false) };
}

fn fresh_root_namespace_id() -> u64 {
    NEXT_ROOT_NAMESPACE_ID.with(|next| {
        let id = next.get();
        next.set(id.saturating_add(1));
        id
    })
}

/// Compact two-bit Bloom mask shared by namespace cache-interest sets.
///
/// False positives cause only an extra cold-map synchronization; the two bits
/// ensure that a recorded name can never be missed by a later precomputed
/// module-write mask.
pub fn namespace_name_interest_mask(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in name.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let first = hash & 63;
    let second = hash.rotate_right(29) & 63;
    (1_u64 << first) | (1_u64 << second)
}

#[derive(Debug)]
enum NamespaceGlobalsProvider {
    Normal(Value),
    Explicit {
        globals: Value,
        locals: Option<Value>,
    },
}

#[derive(Debug)]
struct ActiveNamespaceMirror {
    id: u64,
    regs_ptr: NonNull<Value>,
    regs_len: usize,
    local_index: Weak<HashMap<String, u32>>,
    lifetime: Weak<()>,
}

struct LiveNamespaceMirror {
    regs_ptr: NonNull<Value>,
    regs_len: usize,
    local_index: Rc<HashMap<String, u32>>,
    _lifetime: Rc<()>,
}

/// Non-owning, pointer-free description of one live Script fastlocal.
pub struct NamespaceFastLocalCacheSource {
    pub value: Value,
    pub local_index: Weak<HashMap<String, u32>>,
    pub register: u32,
    pub mirror_epoch: u64,
}

/// RAII proof that one Script register file remains alive for namespace-dict
/// mutation synchronization.
///
/// The guard is created immediately before the VM starts and dropped before
/// its register buffer. The root keeps only weak lifetime/layout handles plus a
/// raw pointer, so neither compiled layouts nor register storage are retained
/// after the Script frame ends.
#[derive(Debug)]
pub struct NamespaceMirrorGuard {
    root_namespace: Weak<RootNamespaceState>,
    id: u64,
    _lifetime: Rc<()>,
}

impl Drop for NamespaceMirrorGuard {
    fn drop(&mut self) {
        let Some(root) = self.root_namespace.upgrade() else {
            return;
        };
        let mut active = root.active_mirrors.borrow_mut();
        let old_len = active.len();
        active.retain(|mirror| mirror.id != self.id);
        if active.len() != old_len {
            root.active_mirror_count
                .set(root.active_mirror_count.get().saturating_sub(1));
            root.fastlocal_mirror_epoch
                .set(root.fastlocal_mirror_epoch.get().saturating_add(1));
        }
    }
}

/// State owned by one root/module namespace and shared by every lexical child.
///
/// A function can outlive the [`Interpreter`](crate::Environment) that created
/// it and can later be invoked through a different interpreter object (imported
/// Python modules do exactly that).  Cache generations and the globals backing
/// therefore belong to the namespace captured by the function, not to whichever
/// interpreter happens to execute it.
#[derive(Debug)]
struct RootNamespaceState {
    id: u64,
    environment_version: Cell<u64>,
    structure_version: Cell<u64>,
    /// Bloom-style interest mask for values that may be held by Environment or
    /// ScriptFastLocal `LoadGlobal` caches in this namespace. Module writes
    /// only need to advance `environment_version` when their precomputed name
    /// mask intersects this root-lifetime set.
    value_cache_interest: Cell<u64>,
    /// Bloom-style interest mask for canonical fallback values that may be
    /// held by a `LoadGlobal` cache in this namespace. A module assignment only
    /// needs to advance `structure_version` when its precomputed name mask
    /// intersects this root-lifetime set. False positives are harmless extra
    /// invalidations; false negatives are impossible because cache
    /// installation registers its mask before publishing the entry.
    fallback_cache_interest: Cell<u64>,
    /// Names that have ever gained a slow EnvValues binding in this root.
    /// Fastlocal writes consult the same Bloom mask and synchronize the map
    /// only on a possible overlap, avoiding a HashMap probe on ordinary module
    /// assignments while preventing stale slow bindings from winning lookup.
    env_binding_interest: Cell<u64>,
    /// Live Script fast-local mirrors for this root. Mutations through any
    /// alias of an exposed namespace dictionary update every matching mirror
    /// at the storage boundary, including mutations performed by a child
    /// Interpreter during an import.
    active_mirrors: RefCell<Vec<ActiveNamespaceMirror>>,
    active_mirror_count: Cell<usize>,
    /// Changes whenever a Script mirror enters or leaves this root. Locator
    /// caches include it so nested or later frames cannot reuse a stale slot.
    fastlocal_mirror_epoch: Cell<u64>,
    next_mirror_id: Cell<u64>,
    /// Dictionary identities registered in the thread-local alias index.
    /// Keeping this reverse index lets `Drop` remove dead roots immediately
    /// instead of leaving every later ordinary dict mutation to probe stale
    /// entries forever.
    alias_identities: RefCell<Vec<usize>>,
    /// Once the globals mapping is exposed, arbitrary aliases can mutate it
    /// without passing through the interpreter's assignment helpers.  Such a
    /// namespace remains uncached for the rest of its lifetime.
    cache_disabled: Cell<bool>,
    /// Lazily allocated so placeholder environments used by interned builtin
    /// callables do not each pay for an otherwise-unused dictionary.
    globals_provider: RefCell<Option<NamespaceGlobalsProvider>>,
    /// Filesystem modules expose the root globals backing through a separate
    /// `PyModule` object. Controlled module/script writes therefore mirror into
    /// that backing even before Python obtains a `__dict__` alias.
    filesystem_module_backing: Cell<bool>,
    /// The filesystem module's provider generation. Once the globals mapping
    /// escapes through `module.__dict__`, `vars(module)`, or
    /// `function.__globals__`, arbitrary dict mutations can bypass
    /// `PyModule::insert_attr`; saturating this token disables module-backed
    /// equality caches at the same boundary as the root LoadGlobal cache.
    filesystem_module_mutation: RefCell<Option<ModuleMutationState>>,
}

impl RootNamespaceState {
    fn new() -> Self {
        Self {
            id: fresh_root_namespace_id(),
            environment_version: Cell::new(0),
            structure_version: Cell::new(0),
            value_cache_interest: Cell::new(0),
            fallback_cache_interest: Cell::new(0),
            env_binding_interest: Cell::new(0),
            active_mirrors: RefCell::new(Vec::new()),
            active_mirror_count: Cell::new(0),
            fastlocal_mirror_epoch: Cell::new(0),
            next_mirror_id: Cell::new(1),
            alias_identities: RefCell::new(Vec::new()),
            cache_disabled: Cell::new(false),
            globals_provider: RefCell::new(None),
            filesystem_module_backing: Cell::new(false),
            filesystem_module_mutation: RefCell::new(None),
        }
    }

    #[inline]
    fn next_generation(generation: u64) -> u64 {
        generation.saturating_add(1)
    }

    #[inline]
    fn bump_environment_version(&self) {
        self.environment_version
            .set(Self::next_generation(self.environment_version.get()));
    }

    #[inline]
    fn bump_structure_version(&self) {
        self.structure_version
            .set(Self::next_generation(self.structure_version.get()));
        self.bump_environment_version();
    }

    fn globals_provider_snapshot(&self) -> (Value, Option<Value>, bool) {
        let mut provider = self.globals_provider.borrow_mut();
        if provider.is_none() {
            *provider = Some(NamespaceGlobalsProvider::Normal(Value::dict(
                PyDict::default(),
            )));
        }
        match provider.as_ref().expect("provider initialized above") {
            NamespaceGlobalsProvider::Normal(globals) => (globals.clone(), None, false),
            NamespaceGlobalsProvider::Explicit { globals, locals } => {
                (globals.clone(), locals.clone(), true)
            }
        }
    }

    fn candidate_is_authoritative_locals(&self, candidate: &Rc<RefCell<PyDict>>) -> bool {
        {
            let provider = self.globals_provider.borrow();
            match provider.as_ref() {
                Some(NamespaceGlobalsProvider::Normal(globals)) => globals
                    .get_dict_rc()
                    .is_some_and(|globals| Rc::ptr_eq(globals, candidate)),
                Some(NamespaceGlobalsProvider::Explicit { globals, locals }) => locals
                    .as_ref()
                    .unwrap_or(globals)
                    .get_dict_rc()
                    .is_some_and(|locals| Rc::ptr_eq(locals, candidate)),
                None => false,
            }
        }
    }

    fn live_mirrors(&self) -> Vec<LiveNamespaceMirror> {
        let mut active = self.active_mirrors.borrow_mut();
        let old_len = active.len();
        active.retain(|mirror| {
            mirror.lifetime.upgrade().is_some() && mirror.local_index.upgrade().is_some()
        });
        self.active_mirror_count.set(active.len());
        if active.len() != old_len {
            self.fastlocal_mirror_epoch
                .set(self.fastlocal_mirror_epoch.get().saturating_add(1));
        }
        active
            .iter()
            .filter_map(|mirror| {
                Some(LiveNamespaceMirror {
                    regs_ptr: mirror.regs_ptr,
                    regs_len: mirror.regs_len,
                    local_index: mirror.local_index.upgrade()?,
                    _lifetime: mirror.lifetime.upgrade()?,
                })
            })
            .collect()
    }

    fn synchronize_alias_key_mutation(&self, candidate: &Rc<RefCell<PyDict>>, name: &str) {
        if !self.candidate_is_authoritative_locals(candidate) {
            return;
        }

        self.bump_structure_version();
        let value = {
            let dict = candidate.borrow();
            dict.get(&crate::object_model::StrKey(name))
                .cloned()
                .unwrap_or_else(Value::unset)
        };
        for mirror in self.live_mirrors() {
            let Some(&slot) = mirror.local_index.get(name) else {
                continue;
            };
            let slot = slot as usize;
            if slot >= mirror.regs_len {
                continue;
            }
            // SAFETY: the live mirror retains the guard lifetime, and the
            // bounds check above covers this slot.
            unsafe {
                *mirror.regs_ptr.add(slot).as_mut() = value.clone();
            }
        }
    }

    fn synchronize_alias_full_mutation(&self, candidate: &Rc<RefCell<PyDict>>) {
        if !self.candidate_is_authoritative_locals(candidate) {
            return;
        }

        self.bump_structure_version();
        for mirror in self.live_mirrors() {
            // Clone every replacement while the dict is immutably borrowed,
            // then drop that borrow before overwriting registers. This keeps a
            // replaced Value's destruction outside the provider borrow.
            let updates: Vec<(usize, Value)> = {
                let dict = candidate.borrow();
                mirror
                    .local_index
                    .iter()
                    .filter_map(|(name, &slot)| {
                        let slot = slot as usize;
                        (slot < mirror.regs_len).then(|| {
                            let value = dict
                                .get(&crate::object_model::StrKey(name))
                                .cloned()
                                .unwrap_or_else(Value::unset);
                            (slot, value)
                        })
                    })
                    .collect()
            };
            for (slot, value) in updates {
                // SAFETY: `NamespaceMirrorGuard` proves the register allocation
                // outlives this entry; `slot < regs_len` was checked above.
                // Script execution uses RegSlice/raw-pointer access exclusively,
                // so no competing `&mut [Value]` noalias reference exists.
                unsafe {
                    *mirror.regs_ptr.add(slot).as_mut() = value;
                }
            }
        }
    }
}

impl Drop for RootNamespaceState {
    fn drop(&mut self) {
        let root_ptr = self as *const Self;
        let identities = self.alias_identities.get_mut();
        if identities.is_empty() {
            return;
        }
        let _ = NAMESPACE_ALIAS_OWNERS.try_with(|owners| {
            let mut owners = owners.borrow_mut();
            for identity in identities.drain(..) {
                let remove_entry = if let Some(providers) = owners.get_mut(&identity) {
                    providers.retain(|provider| provider.as_ptr() != root_ptr);
                    providers.is_empty()
                } else {
                    false
                };
                if remove_entry {
                    owners.remove(&identity);
                }
            }
            if owners.is_empty() {
                let _ = HAS_NAMESPACE_ALIASES.try_with(|active| active.set(false));
            }
        });
    }
}

#[inline]
fn namespace_dict_identity(dict: &Rc<RefCell<PyDict>>) -> usize {
    Rc::as_ptr(dict) as usize
}

fn register_namespace_alias(root: &Rc<RootNamespaceState>, value: &Value) {
    let Some(dict) = value.get_dict_rc() else {
        return;
    };
    let identity = namespace_dict_identity(dict);
    NAMESPACE_ALIAS_OWNERS.with(|owners| {
        let mut owners = owners.borrow_mut();
        let providers = owners.entry(identity).or_default();
        providers.retain(|provider| provider.upgrade().is_some());
        if !providers.iter().any(|provider| {
            provider
                .upgrade()
                .is_some_and(|provider| Rc::ptr_eq(&provider, root))
        }) {
            providers.push(Rc::downgrade(root));
        }
    });
    {
        let mut identities = root.alias_identities.borrow_mut();
        if !identities.contains(&identity) {
            identities.push(identity);
        }
    }
    HAS_NAMESPACE_ALIASES.with(|active| active.set(true));
}

#[inline]
pub(crate) fn namespace_alias_tracking_active() -> bool {
    HAS_NAMESPACE_ALIASES.with(Cell::get)
}

fn namespace_alias_roots(dict: &Rc<RefCell<PyDict>>) -> Vec<Rc<RootNamespaceState>> {
    if !HAS_NAMESPACE_ALIASES.with(Cell::get) {
        return Vec::new();
    }
    let identity = namespace_dict_identity(dict);
    NAMESPACE_ALIAS_OWNERS.with(|owners| {
        let mut owners = owners.borrow_mut();
        let Some(providers) = owners.get_mut(&identity) else {
            return Vec::new();
        };
        let roots: Vec<_> = providers.iter().filter_map(Weak::upgrade).collect();
        providers.retain(|provider| provider.upgrade().is_some());
        if providers.is_empty() {
            owners.remove(&identity);
        }
        if owners.is_empty() {
            HAS_NAMESPACE_ALIASES.with(|active| active.set(false));
        }
        roots
    })
}

/// Notify every live root whose Python-visible namespace uses this dictionary
/// backing that one exact string key changed. Called only after the mutable
/// dictionary borrow has been dropped.
pub(crate) fn notify_namespace_dict_key_mutation(dict: &Rc<RefCell<PyDict>>, name: &str) {
    for root in namespace_alias_roots(dict) {
        root.synchronize_alias_key_mutation(dict, name);
    }
}

/// Notify namespace owners after an opaque or batched dictionary mutation.
/// This is intentionally the cold full-refresh path; single-key operations
/// use [`notify_namespace_dict_key_mutation`] to avoid O(locals) work.
pub(crate) fn notify_namespace_dict_mutation(dict: &Rc<RefCell<PyDict>>) {
    for root in namespace_alias_roots(dict) {
        root.synchronize_alias_full_mutation(dict);
    }
}
