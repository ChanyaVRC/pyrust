// Cache-safe global resolution policy.
/// Whether an uncached global-name result may be stored in an opcode cache.
///
/// Namespace resolution decides cache safety; the fast-path domain decides how
/// the corresponding cache is probed and updated.
pub(crate) enum GlobalResolutionCache {
    None,
    Environment(u64),
    ScriptFastLocal {
        environment_version: u64,
        mirror_epoch: u64,
        local_index: std::rc::Weak<HashMap<String, u32>>,
        register: u32,
    },
    Builtin(ModuleMutationState),
}

pub(crate) struct GlobalResolution {
    pub(crate) value: Value,
    pub(crate) cache: GlobalResolutionCache,
}

impl Interpreter {
    /// Resolve a global after the opcode caches have missed.
    ///
    /// This owns Python namespace precedence and error semantics. It deliberately
    /// knows nothing about the bytecode cache layout.
    pub(crate) fn resolve_global_uncached(
        &mut self,
        name: &str,
        is_free: bool,
        current_environment_version: u64,
        namespace: &GlobalNamespace,
    ) -> Result<GlobalResolution> {
        if let Some(value) = self.lookup_name_inner(name, is_free)? {
            // Only module-environment mutations are tracked by the global value
            // version. An intermediate class/type-param environment may shadow a
            // same-named module binding and must never be cached under that version.
            let in_module_environment =
                self.active_frame_declares_global(name) || self.env.borrow().parent.is_none();
            return Ok(GlobalResolution {
                value,
                cache: if in_module_environment {
                    GlobalResolutionCache::Environment(current_environment_version)
                } else {
                    GlobalResolutionCache::None
                },
            });
        }

        // Pick up writes performed through globals()["name"]. These bypass the
        // environment version, so the result is intentionally not cacheable.
        if let Some(value) = namespace
            .globals()
            .dict_with(|dict| dict.get(&StrKey(name)).cloned())
            .flatten()
        {
            return Ok(GlobalResolution {
                value,
                cache: GlobalResolutionCache::None,
            });
        }

        if !namespace.is_explicit()
            && let Some(source) = module_env(&self.env)
                .borrow()
                .namespace_fastlocal_cache_source(name)
        {
            return Ok(GlobalResolution {
                value: source.value,
                cache: GlobalResolutionCache::ScriptFastLocal {
                    environment_version: current_environment_version,
                    mirror_epoch: source.mirror_epoch,
                    local_index: source.local_index,
                    register: source.register,
                },
            });
        }

        self.resolve_global_via_builtins(name, namespace.globals())
    }

    /// Resolve the builtins tail of Python global lookup.
    ///
    /// A `globals["__builtins__"]` module is a namespace provider, not merely
    /// a signal that the static registry may be consulted.  Its attributes
    /// are therefore authoritative: mutation and deletion must affect the
    /// next global lookup exactly as they do in CPython.
    #[cold]
    #[inline(never)]
    fn resolve_global_via_builtins(&self, name: &str, globals: &Value) -> Result<GlobalResolution> {
        let configured = globals
            .dict_with(|dict| dict.get(&StrKey("__builtins__")).cloned())
            .flatten();
        let builtins_value = configured.unwrap_or_else(cached_builtins_module);
        match builtins_value.kind() {
            ValueKind::Dict(_) => {
                let value = builtins_value
                    .dict_with(|dict| dict.get(&StrKey(name)).cloned())
                    .flatten()
                    .ok_or_else(|| undefined_name(name))?;
                Ok(GlobalResolution {
                    value,
                    // A user-provided dictionary can mutate through aliases;
                    // keep it off the opcode cache.
                    cache: GlobalResolutionCache::None,
                })
            }
            ValueKind::PyModule(module) => {
                let (value, provider_state) = {
                    let module = module.borrow();
                    let value = module
                        .get_attr_value(name)
                        .filter(|value| !value.is_unset())
                        .ok_or_else(|| undefined_name(name))?;
                    (value, module.mutation_state())
                };
                let cache = if is_cached_builtins_module(module) {
                    GlobalResolutionCache::Builtin(provider_state)
                } else {
                    // Custom modules are authoritative but intentionally
                    // uncacheable.  Only the canonical provider has a stable
                    // lifecycle and a deliberately tracked mutation surface.
                    GlobalResolutionCache::None
                };
                Ok(GlobalResolution { value, cache })
            }
            _ => Err(pyrust_core::type_err!(
                "'{}' object is not subscriptable",
                value_type_name_str(&builtins_value),
            )),
        }
    }
}

fn undefined_name(name: &str) -> PyError {
    PyError::name_error(
        "NameError",
        format!("name '{name}' is not defined"),
        Some(name.to_string()),
    )
}

/// Decide whether an unbound `LoadGlobal` / `LoadCell` read denotes a captured
/// free variable or a plain local. PEP 709 inlined comprehensions retain the
/// latter classification for names local to their inlining target.
#[inline]
pub(crate) fn comp_read_is_free(code: &crate::bytecode::FnCode, name: &str) -> bool {
    match &code.comp_enclosing_locals {
        Some(locals) => !locals.contains(&pyrust_core::Symbol::intern(name)),
        None => true,
    }
}
