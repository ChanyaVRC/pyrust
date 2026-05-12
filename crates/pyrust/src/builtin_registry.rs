//! Compile-time registry of built-in Python callables.
//!
//! Built-ins are declared in one of two ways (see the `pyrust-derive` crate):
//!
//! - **`pyrust_module! { … }`** (function-like, primary) — declares a
//!   whole built-in module's callables and constants at once.  Each `fn`
//!   inside the macro expands to a `BuiltinReg` and is collected into the
//!   per-module `regs()` slice.  Function names that collide with Rust
//!   keywords use the `#[py_name = "..."]` override.
//! - **`#[pyfunction(name = "module.fn")]`** (attribute, one-off
//!   fallback) — for migrating a single arm without spinning up a module
//!   file.
//!
//! Both forms feed [`crate::builtin_modules::all_regs`], which this module
//! collects into a single sorted static registry exposed via [`lookup`]
//! for O(log n) name-to-dispatch resolution.
//!
//! Compared to the previous `match ValueKind::BuiltinFunction("name") => …`
//! cascade, this design:
//! - decouples the *declaration* of a built-in from its dispatch arm,
//! - dispatches through a `fn` pointer instead of a string match,
//! - has zero per-call allocation (registry is `&'static`).
//!
//! Binary-searched at lookup time — for our scale (~70 builtins) faster
//! than a `HashMap` (no hashing).

use crate::Interpreter;
use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

/// Unified dispatch signature for built-in callables.
///
/// `interp` gives access to the interpreter for built-ins that re-enter
/// (e.g. `map`, `reduce`, `sorted` with a key).  Pure built-ins like
/// `abs` or `math.sqrt` ignore it.
pub type BuiltinDispatchFn = fn(&mut Interpreter, &[ExpandedCallArg]) -> Result<Value>;

/// One entry in the registry — emitted by `pyrust_module!` (one per fn
/// inside the macro body) or by `#[pyfunction(name = …)]`.
#[derive(Copy, Clone)]
pub struct BuiltinReg {
    pub name: &'static str,
    pub dispatch: BuiltinDispatchFn,
}

/// Look up the dispatcher for a Python-level built-in name.  Returns
/// `None` if no built-in by that name is registered, in which case the
/// caller falls back to the legacy match cascade.
#[inline]
pub fn lookup(name: &str) -> Option<BuiltinDispatchFn> {
    REGISTRY
        .binary_search_by_key(&name, |r| r.name)
        .ok()
        .map(|i| REGISTRY[i].dispatch)
}

/// The full registry.  Constructed via [`std::sync::LazyLock`] from
/// every module's `REGS` slice (collected by
/// `crate::builtin_modules::all_regs`) so adding a built-in
/// module touches **only** the `pyrust_builtin_modules!` invocation in
/// `builtin_modules/mod.rs` — this file needs no edits.
static REGISTRY: std::sync::LazyLock<Vec<BuiltinReg>> = std::sync::LazyLock::new(|| {
    let mut all = crate::builtin_modules::all_regs();
    all.sort_by_key(|r| r.name);
    // `assert!` (not `debug_assert!`) — a duplicate registration would
    // silently make `lookup()` ambiguous in release builds.  This runs
    // once on first lookup, so the cost is negligible.
    if let Some(w) = all.windows(2).find(|w| w[0].name >= w[1].name) {
        panic!(
            "duplicate built-in name in registry: `{}` (sort produced `{}` followed by `{}`)",
            w[0].name, w[0].name, w[1].name,
        );
    }
    all
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ValueKind;

    #[test]
    fn lookup_finds_a_known_builtin() {
        // math.sqrt is one of the first migrated builtins; it must be present.
        assert!(
            lookup("math.sqrt").is_some(),
            "math.sqrt should be registered"
        );
    }

    #[test]
    fn lookup_misses_an_unknown_name() {
        assert!(lookup("absolutely-not-a-builtin").is_none());
    }

    #[test]
    fn registry_is_sorted_and_unique() {
        let names: Vec<&'static str> = REGISTRY.iter().map(|r| r.name).collect();
        for w in names.windows(2) {
            assert!(
                w[0] < w[1],
                "registry not sorted/unique at {:?} >= {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn flat_namespace_builtins_register_without_prefix() {
        // `@flat builtins` mode means top-level Python builtins
        // (`abs`, `len`, `print`, …) register under their short name,
        // not `builtins.abs`.  Picking a handful at random pins the contract.
        for name in ["abs", "len", "print", "type", "super"] {
            assert!(
                lookup(name).is_some(),
                "expected flat builtin {name:?} in registry"
            );
            assert!(
                lookup(&format!("builtins.{name}")).is_none(),
                "flat builtin {name:?} must NOT be registered with `builtins.` prefix"
            );
        }
    }

    #[test]
    fn registry_name_and_module_attr_share_one_pointer() {
        // The Copilot review flagged that `pyrust_module!` used to emit
        // three independent `Box::leak` sites per built-in (one for
        // `FN_NAME`, one for `BuiltinReg.name`, one for the
        // `Value::builtin_function(...)` in the module attrs).  After the
        // fix, all three consumers read from a single per-fn
        // `LazyLock<&'static str>`, so the `&'static str` stored in the
        // registry must be **the exact same pointer** as the one bound
        // into the imported `PyModule`.  Compare with `std::ptr::eq` —
        // string equality alone wouldn't catch a regression to multiple
        // leaks.
        let registered = lookup("math.sqrt").map(|_| {
            REGISTRY
                .iter()
                .find(|r| r.name == "math.sqrt")
                .expect("just looked up")
                .name
        });
        let registered = registered.expect("math.sqrt must be registered");

        let module =
            crate::builtin_modules::load_builtin_module("math").expect("import math must resolve");
        let module_attr_name: &'static str = match module.kind() {
            ValueKind::PyModule(m) => match m.borrow().attrs.get("sqrt").map(|v| v.kind()) {
                Some(ValueKind::BuiltinFunction(s)) => s,
                _ => panic!("math.sqrt attr is not a BuiltinFunction"),
            },
            _ => panic!("import math did not return a PyModule"),
        };

        assert!(
            std::ptr::eq(registered, module_attr_name),
            "BuiltinReg.name and PyModule attr name must point to the same \
             leaked str (got {registered:p} vs {module_attr_name:p})",
        );
    }

    #[test]
    fn duplicate_registration_panics_at_init() {
        // The uniqueness check fires when REGISTRY is built (first lookup).
        // We can't *cause* a duplicate from the outside — the registry is
        // populated from compile-time-fixed `regs()` slices — so this test
        // is necessarily a smoke check that registry init has run cleanly.
        // The behavioural assertion (`assert!`, not `debug_assert!`) lives
        // in the static initialiser; a duplicate would have panicked
        // before any other test could reach `lookup()`.
        assert!(lookup("abs").is_some());
    }
}
