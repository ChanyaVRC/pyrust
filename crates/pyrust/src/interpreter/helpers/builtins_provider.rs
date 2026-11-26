// Thread-local justification:
// (a) Why `thread_local!`: Holds a `!Send` NaN-boxed `Value` (`Rc<RefCell<PyModule>>`)
//     for the `builtins` module so script startup clones one `Rc` instead of
//     rebuilding the namespace.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread share the cached `builtins` module attrs map unless a script
//     replaces `__builtins__`; separate threads own independent `builtins`
//     modules.
thread_local! {
    /// Per-thread cache of the `builtins` module value.
    /// `seed_module_dunders` clones this `Value` (one `Rc` increment) instead
    /// of rebuilding the module's `ModuleAttrs` map from scratch on every
    /// script invocation.
    static BUILTINS_MODULE_CACHE: Value =
        crate::builtin_modules::load_builtin_module("builtins").unwrap_or_else(Value::none);
}

/// Return a clone of the thread-local `builtins` module.  O(1) on subsequent
/// calls — clones the `Rc<RefCell<PyModule>>` reference (attrs map is shared).
///
/// The module is built in a single pass that stores only function tokens and
/// constants.  The class objects CPython also exposes there — the primitive
/// type singletons (`int`, `str`, `list`, …) and the built-in exception
/// hierarchy (issue #1255) — are deferred attributes of the module: each is
/// resolved to its per-thread singleton on first lookup, and all of them are
/// materialized in insertion order once the whole namespace is observed
/// (issue #3294).
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
