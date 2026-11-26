//! Global-name resolution cache protocol.

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::value::Value;

/// Final cached resolution for one `LoadGlobal` name slot.
///
/// A name resolves either from the module environment or from the canonical
/// `builtins` module, so both forms share one mutually exclusive cache entry.
/// `FnCode` is reusable across interpreters and explicit `eval`/`exec`
/// namespaces, so every variant guards the root namespace identity. Environment
/// hits additionally guard the value generation; root-slot hits guard the
/// structure generation and revalidate their slot on every read; builtin-module
/// hits guard the namespace structure generation and the shared module's
/// mutation state.
/// Cached heap identities are held weakly so the code object cannot keep an
/// obsolete global alive (or close a function -> code -> cache -> function
/// cycle). A dead weak value is a normal miss; representations without a
/// weak-capable backing leave the slot empty.
/// Explicit dictionaries and namespaces exposed through `globals()` never
/// populate this cache. Saturated namespace or provider generations produce
/// `Empty` and can never validate a hit.
#[derive(Clone)]
pub(crate) enum GlobalCacheEntry {
    Empty,
    Environment {
        namespace_id: u64,
        environment_version: u64,
        value: pyrust_core::WeakValueCache,
    },
    /// Self-validating locator for a binding stored in the root namespace map
    /// (#3423).
    ///
    /// The entry names the binding's slot by index and key identity and reads
    /// the current value through it, so overwriting the binding needs no cache
    /// invalidation at all: the next hit simply observes the new value. It
    /// holds the key string only, never the bound value. Removing or
    /// re-inserting the binding changes the slot's key, which is an ordinary
    /// miss; the structure generation additionally rejects the entry while a
    /// PEP 695 scope could shadow the name.
    ///
    /// A hit costs one more dependent load than the value form, so this lane
    /// is used only where the value form is unavailable: for a name some
    /// `StoreGlobal` cache overwrites in place, and for a value that
    /// [`pyrust_core::WeakValueCache`] cannot hold without owning it.
    EnvironmentSlot {
        namespace_id: u64,
        structure_version: u64,
        index: u32,
        key: Rc<str>,
    },
    /// Pointer-free locator for a value held in a live Script register file.
    ///
    /// This lane avoids reconstructing an opaque Value wrapper on every hit.
    /// It retains neither the register storage nor its layout and must validate
    /// the root's mirror epoch before reading through Environment.
    ScriptFastLocal {
        namespace_id: u64,
        environment_version: u64,
        mirror_epoch: u64,
        local_index: Weak<HashMap<String, u32>>,
        register: u32,
        /// Allocation-free weak/inline representation when available. The
        /// mirror epoch still guards it; otherwise the live slot locator is
        /// used to avoid reconstructing an opaque wrapper.
        value: Option<pyrust_core::WeakValueCache>,
    },
    BuiltinModule {
        namespace_id: u64,
        structure_version: u64,
        provider_state: pyrust_core::ModuleMutationState,
        provider_version: u64,
        value: pyrust_core::WeakValueCache,
    },
}

/// Precompute a compact two-bit Bloom mask for one global name.
///
/// Root namespaces OR these masks into the appropriate value/fallback interest
/// set before publishing a `LoadGlobal` cache. `SyncModuleGlobal` can then
/// decide which generation a write may invalidate with integer operations and
/// no Python-name lookup. Hash collisions only cause harmless extra
/// invalidations.
pub(crate) fn global_cache_interest_mask(name: &str) -> u64 {
    pyrust_core::namespace_name_interest_mask(name)
}

/// Cached target of one `StoreGlobal` name slot (#3423).
///
/// `RootSlot` records that the name was last assigned into an existing root
/// namespace binding, located by a self-validating
/// [`pyrust_core::NamespaceStoreSlot`]. A hit overwrites that binding in place
/// instead of repeating name resolution, insertion, and mirror discovery.
///
/// `declared_in` is the proof that a frame whose lexical environment is *not*
/// the root still targets the root: the active function's `global`-name set,
/// which contained this name when the entry was filled. Holding the set keeps
/// its allocation from being reused, so pointer equality with the active
/// function's set proves the declaration. It is `None` when the entry was
/// filled from the root environment itself, where a `global` declaration and a
/// plain module-scope store write the same binding.
#[derive(Clone, Debug, Default)]
pub(crate) enum GlobalStoreCacheEntry {
    #[default]
    Empty,
    RootSlot {
        slot: pyrust_core::NamespaceStoreSlot,
        declared_in: Option<pyrust_core::NameSet>,
    },
}

impl GlobalCacheEntry {
    #[cfg(test)]
    #[inline]
    pub(crate) fn namespace_id(&self) -> Option<u64> {
        match self {
            Self::Empty => None,
            Self::Environment { namespace_id, .. }
            | Self::EnvironmentSlot { namespace_id, .. }
            | Self::ScriptFastLocal { namespace_id, .. }
            | Self::BuiltinModule { namespace_id, .. } => Some(*namespace_id),
        }
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn lookup(
        &self,
        namespace_id: u64,
        environment_version: u64,
        structure_version: u64,
    ) -> Option<Value> {
        self.lookup_with_fastlocal(
            namespace_id,
            environment_version,
            structure_version,
            u64::MAX,
            |_, _, _| None,
            |_, _| None,
        )
    }

    /// Validate this entry against the root namespace snapshot and produce the
    /// cached resolution.
    ///
    /// `script_lookup` reads a live Script register through its pointer-free
    /// locator; `slot_lookup` reads a root binding through its `(index, key)`
    /// slot locator. Both return `None` when their locator went stale.
    #[inline]
    pub(crate) fn lookup_with_fastlocal(
        &self,
        namespace_id: u64,
        environment_version: u64,
        structure_version: u64,
        current_mirror_epoch: u64,
        script_lookup: impl FnOnce(u64, &Weak<HashMap<String, u32>>, u32) -> Option<Value>,
        slot_lookup: impl FnOnce(u32, &Rc<str>) -> Option<Value>,
    ) -> Option<Value> {
        match self {
            Self::Empty => None,
            Self::Environment {
                namespace_id: cached_namespace_id,
                environment_version: cached_environment_version,
                value,
            } => {
                if *cached_namespace_id == namespace_id
                    && *cached_environment_version == environment_version
                {
                    value.upgrade()
                } else {
                    None
                }
            }
            Self::EnvironmentSlot {
                namespace_id: cached_namespace_id,
                structure_version: cached_structure_version,
                index,
                key,
            } => {
                if *cached_namespace_id == namespace_id
                    && *cached_structure_version == structure_version
                {
                    slot_lookup(*index, key)
                } else {
                    None
                }
            }
            Self::ScriptFastLocal {
                namespace_id: cached_namespace_id,
                environment_version: cached_environment_version,
                mirror_epoch,
                local_index,
                register,
                value,
            } => {
                if *cached_namespace_id == namespace_id
                    && *cached_environment_version == environment_version
                    && *mirror_epoch == current_mirror_epoch
                {
                    value
                        .as_ref()
                        .and_then(|value| value.upgrade())
                        .or_else(|| script_lookup(*mirror_epoch, local_index, *register))
                } else {
                    None
                }
            }
            Self::BuiltinModule {
                namespace_id: cached_namespace_id,
                structure_version: cached_structure_version,
                provider_state,
                provider_version,
                value,
            } => {
                if *cached_namespace_id == namespace_id
                    && *cached_structure_version == structure_version
                    && provider_state.matches_cache_version(*provider_version)
                {
                    value.upgrade()
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn environment(namespace_id: u64, environment_version: u64, value: &Value) -> Self {
        if namespace_id == u64::MAX || environment_version == u64::MAX {
            return Self::Empty;
        }
        let Some(value) = pyrust_core::WeakValueCache::new(value) else {
            return Self::Empty;
        };
        Self::Environment {
            namespace_id,
            environment_version,
            value,
        }
    }

    /// Cache a root-namespace binding by slot locator rather than by value.
    pub(crate) fn environment_slot(
        namespace_id: u64,
        structure_version: u64,
        index: u32,
        key: Rc<str>,
    ) -> Self {
        if namespace_id == u64::MAX || structure_version == u64::MAX {
            return Self::Empty;
        }
        Self::EnvironmentSlot {
            namespace_id,
            structure_version,
            index,
            key,
        }
    }

    pub(crate) fn script_fastlocal(
        namespace_id: u64,
        environment_version: u64,
        mirror_epoch: u64,
        local_index: Weak<HashMap<String, u32>>,
        register: u32,
        value: &Value,
    ) -> Self {
        if namespace_id == u64::MAX || environment_version == u64::MAX || mirror_epoch == u64::MAX {
            return Self::Empty;
        }
        let value = pyrust_core::WeakValueCache::new(value)
            .filter(pyrust_core::WeakValueCache::upgrade_is_allocation_free);
        Self::ScriptFastLocal {
            namespace_id,
            environment_version,
            mirror_epoch,
            local_index,
            register,
            value,
        }
    }

    pub(crate) fn builtin_module(
        namespace_id: u64,
        structure_version: u64,
        provider_state: pyrust_core::ModuleMutationState,
        value: &Value,
    ) -> Self {
        if namespace_id == u64::MAX || structure_version == u64::MAX {
            return Self::Empty;
        }
        let Some(provider_version) = provider_state.cache_version() else {
            return Self::Empty;
        };
        let Some(value) = pyrust_core::WeakValueCache::new(value) else {
            return Self::Empty;
        };
        Self::BuiltinModule {
            namespace_id,
            structure_version,
            provider_state,
            provider_version,
            value,
        }
    }
}

impl std::fmt::Debug for GlobalCacheEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Environment {
                namespace_id,
                environment_version,
                ..
            } => formatter
                .debug_struct("Environment")
                .field("namespace_id", namespace_id)
                .field("environment_version", environment_version)
                .finish_non_exhaustive(),
            Self::EnvironmentSlot {
                namespace_id,
                structure_version,
                index,
                key,
            } => formatter
                .debug_struct("EnvironmentSlot")
                .field("namespace_id", namespace_id)
                .field("structure_version", structure_version)
                .field("index", index)
                .field("key", key)
                .finish(),
            Self::ScriptFastLocal {
                namespace_id,
                environment_version,
                mirror_epoch,
                register,
                ..
            } => formatter
                .debug_struct("ScriptFastLocal")
                .field("namespace_id", namespace_id)
                .field("environment_version", environment_version)
                .field("mirror_epoch", mirror_epoch)
                .field("register", register)
                .finish_non_exhaustive(),
            Self::BuiltinModule {
                namespace_id,
                structure_version,
                provider_version,
                ..
            } => formatter
                .debug_struct("BuiltinModule")
                .field("namespace_id", namespace_id)
                .field("structure_version", structure_version)
                .field("provider_version", provider_version)
                .finish_non_exhaustive(),
        }
    }
}
