thread_local! {
    /// Per-thread cache of the `builtins` module value.
    /// `seed_module_dunders` clones this `Value` (one `Rc` increment) instead
    /// of rebuilding the module's ~136-entry `ModuleAttrs` map from scratch on
    /// every script invocation.
    static BUILTINS_MODULE_CACHE: Value = {
        let module = crate::builtin_modules::load_builtin_module("builtins")
            .unwrap_or_else(Value::none);
        crate::builtin_modules::prepare_builtins_module(&module);
        module
    };
}

/// Return a clone of the thread-local `builtins` module.  O(1) on subsequent
/// calls — clones the `Rc<RefCell<PyModule>>` reference (attrs map is shared).
///
/// On the very first call per thread, applies post-processing to populate
/// the module attrs with:
///   - Primitive type `PyClass` singletons (`int`, `str`, `list`, …), replacing
///     the `BuiltinFunction(name)` tokens that `pyrust_module!` emits.
///   - Built-in exception class `PyClass` singletons (`ValueError`, `TypeError`,
///     …), which CPython exposes as attributes of the `builtins` module
///     (issue #1255).
///
/// The mutation is applied once and is shared by all future callers because
/// every `Value::clone` of a `PyModule` shares the same `Rc<RefCell<PyModule>>`.
pub(crate) fn cached_builtins_module() -> Value {
    BUILTINS_MODULE_CACHE.with(Value::clone)
}

/// Whether `candidate` is the canonical per-thread `builtins` provider.
///
/// Global-name caching is deliberately limited to this module.  Arbitrary
/// modules installed as `globals["__builtins__"]` remain authoritative but
/// uncacheable, because their lifetime and mutation surface are user-owned.
pub(crate) fn is_cached_builtins_module(candidate: &Rc<RefCell<PyModule>>) -> bool {
    BUILTINS_MODULE_CACHE.with(|cached| {
        let ValueKind::PyModule(module) = cached.kind() else {
            return false;
        };
        Rc::ptr_eq(module, candidate)
    })
}
