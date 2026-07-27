//! Global-name resolution cache protocol.

use std::collections::HashMap;
use std::rc::Weak;

use crate::value::Value;

/// Final cached resolution for one `LoadGlobal` name slot.
///
/// A name resolves either from the module environment or from the canonical
/// `builtins` module, so both forms share one mutually exclusive cache entry.
/// `FnCode` is reusable across interpreters and explicit `eval`/`exec`
/// namespaces, so both variants guard the root namespace identity. Environment
/// hits additionally guard the value generation; builtin-module hits guard the
/// namespace structure generation and the shared module's mutation state.
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

impl GlobalCacheEntry {
    #[cfg(test)]
    #[inline]
    pub(crate) fn namespace_id(&self) -> Option<u64> {
        match self {
            Self::Empty => None,
            Self::Environment { namespace_id, .. }
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
        )
    }

    #[inline]
    pub(crate) fn lookup_with_fastlocal(
        &self,
        namespace_id: u64,
        environment_version: u64,
        structure_version: u64,
        current_mirror_epoch: u64,
        script_lookup: impl FnOnce(u64, &Weak<HashMap<String, u32>>, u32) -> Option<Value>,
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
