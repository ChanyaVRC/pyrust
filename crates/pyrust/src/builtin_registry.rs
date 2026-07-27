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

/// "Vectorcall"-style fast dispatch for a typed built-in with no `*args` /
/// `**kwargs` / keyword-only parameters: the caller (a positional `Insn::Call`)
/// passes the argument *values* directly as a slice, so the fast entry skips
/// the `ExpandedCallArg` buffer, the keyword-argument validation, and the arity
/// pre-check that the general [`BuiltinDispatchFn`] pays on every call. The
/// caller guarantees `min_arity <= args.len() <= max_arity`.
pub type BuiltinFastDispatchFn = fn(&mut Interpreter, &[Value]) -> Result<Value>;

/// Python-visible callable category carried by a registry entry.
///
/// A dot in the internal dispatch key is not semantic: both a module function
/// (`math.sqrt`) and an unbound type method (`list.append`) use dotted keys.
/// Consumers must use this tag instead of reparsing [`BuiltinReg::name`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BuiltinCallableKind {
    ModuleFunction,
    MethodDescriptor,
}

/// Presentation metadata for a registered callable.
///
/// `qualname` is the name declared inside `pyrust_module!`: `sqrt` for a
/// module function and `Counter.__getitem__` for a type member.  Some legacy
/// flat-builtin declarations include the module prefix in that string
/// (`builtins.TypeAliasType.__repr__`); accessors normalize it without
/// allocating. `declaring_module` remains available for module-function
/// `__module__` and for diagnostics.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BuiltinCallableMetadata {
    pub kind: BuiltinCallableKind,
    pub declaring_module: &'static str,
    pub qualname: &'static str,
    canonical_owner: Option<pyrust_core::CanonicalClassTag>,
}

impl BuiltinCallableMetadata {
    pub const fn module_function(declaring_module: &'static str, qualname: &'static str) -> Self {
        Self {
            kind: BuiltinCallableKind::ModuleFunction,
            declaring_module,
            qualname,
            canonical_owner: None,
        }
    }

    pub const fn method_descriptor(declaring_module: &'static str, qualname: &'static str) -> Self {
        Self {
            kind: BuiltinCallableKind::MethodDescriptor,
            declaring_module,
            qualname,
            canonical_owner: None,
        }
    }

    /// Construct a descriptor declared by an interpreter-owned canonical
    /// class.  The derive macro emits this form for primitive/object owners so
    /// slot dispatch can validate receivers without reparsing a qualified
    /// callable name.
    pub const fn canonical_method_descriptor(
        declaring_module: &'static str,
        qualname: &'static str,
        owner: pyrust_core::CanonicalClassTag,
    ) -> Self {
        Self {
            kind: BuiltinCallableKind::MethodDescriptor,
            declaring_module,
            qualname,
            canonical_owner: Some(owner),
        }
    }

    /// Python `__qualname__`, excluding a redundant registration-module prefix.
    pub fn python_qualname(self) -> &'static str {
        if self.kind == BuiltinCallableKind::MethodDescriptor
            && self.qualname.starts_with(self.declaring_module)
            && self
                .qualname
                .as_bytes()
                .get(self.declaring_module.len())
                .is_some_and(|separator| *separator == b'.')
        {
            &self.qualname[self.declaring_module.len() + 1..]
        } else {
            self.qualname
        }
    }

    /// Python `__name__`, derived from the declared qualname.
    pub fn python_name(self) -> &'static str {
        self.python_qualname()
            .rsplit_once('.')
            .map_or(self.python_qualname(), |(_, name)| name)
    }

    /// Python `__module__`; method descriptors do not expose this attribute.
    pub const fn python_module(self) -> Option<&'static str> {
        match self.kind {
            BuiltinCallableKind::ModuleFunction => Some(self.declaring_module),
            BuiltinCallableKind::MethodDescriptor => None,
        }
    }

    /// Descriptor owner (`list`, `Counter`, …), when this is a type member.
    #[allow(dead_code)] // Public metadata API; most dispatchers need the typed tag below.
    pub fn owner(self) -> Option<&'static str> {
        match self.kind {
            BuiltinCallableKind::ModuleFunction => None,
            BuiltinCallableKind::MethodDescriptor => self
                .python_qualname()
                .rsplit_once('.')
                .map(|(owner, _)| owner),
        }
    }

    /// Immutable class identity for primitive/object descriptors.
    ///
    /// Named non-canonical owners such as `Counter` intentionally return
    /// `None`; callers that only need Python presentation should use
    /// [`Self::owner`].
    pub const fn descriptor_owner_tag(self) -> Option<pyrust_core::CanonicalClassTag> {
        match self.kind {
            BuiltinCallableKind::ModuleFunction => None,
            BuiltinCallableKind::MethodDescriptor => self.canonical_owner,
        }
    }
}

/// One entry in the registry — emitted by `pyrust_module!` (one per fn
/// inside the macro body) or by `#[pyfunction(name = …)]`.
#[derive(Copy, Clone)]
pub struct BuiltinReg {
    pub name: &'static str,
    pub dispatch: BuiltinDispatchFn,
    pub metadata: BuiltinCallableMetadata,
    /// Optional fast entry for a positional call (see [`BuiltinFastDispatchFn`]).
    /// `None` for legacy-dialect builtins and any typed builtin with `*args` /
    /// `**kwargs` / keyword-only params. Valid only for `min_arity..=max_arity`
    /// positional arguments.
    pub fast: Option<BuiltinFastDispatchFn>,
    pub min_arity: u8,
    pub max_arity: u8,
}

/// Look up the dispatcher for a Python-level built-in name.  Returns
/// `None` if no built-in by that name is registered, in which case the
/// caller falls back to the legacy match cascade.
#[inline]
pub fn lookup(name: &str) -> Option<BuiltinDispatchFn> {
    lookup_registration(name).map(|registration| registration.dispatch)
}

/// Look up the complete immutable registration for a built-in callable.
#[inline]
pub fn lookup_registration(name: &str) -> Option<&'static BuiltinReg> {
    REGISTRY
        .binary_search_by_key(&name, |registration| registration.name)
        .ok()
        .map(|index| &REGISTRY[index])
}

/// Look up typed presentation/ownership metadata for a built-in callable.
#[inline]
pub fn lookup_metadata(name: &str) -> Option<BuiltinCallableMetadata> {
    lookup_registration(name).map(|registration| registration.metadata)
}

/// Look up the interned `&'static str` name for a registered built-in.
///
/// The registry stores the canonical `&'static str` for each built-in (see
/// [`BuiltinReg::name`]).  This function returns that pointer so callers can
/// construct a `Value::builtin_function(name)` without allocating a new
/// `&'static str` from a heap-leaked `Box<str>`.
///
/// Returns `None` if no built-in by that name is registered.
#[inline]
pub fn lookup_name(name: &str) -> Option<&'static str> {
    lookup_registration(name).map(|registration| registration.name)
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

    fn module_attr(module: &Value, name: &str) -> Value {
        let ValueKind::PyModule(module) = module.kind() else {
            panic!("expected PyModule");
        };
        module
            .borrow()
            .attrs
            .get(name)
            .unwrap_or_else(|| panic!("module lacks {name:?}"))
            .clone()
    }

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
    fn callable_metadata_distinguishes_module_functions_and_descriptors() {
        let sqrt = lookup_metadata("math.sqrt").expect("math.sqrt must be registered");
        assert_eq!(sqrt.kind, BuiltinCallableKind::ModuleFunction);
        assert_eq!(sqrt.python_name(), "sqrt");
        assert_eq!(sqrt.python_qualname(), "sqrt");
        assert_eq!(sqrt.python_module(), Some("math"));
        assert_eq!(sqrt.owner(), None);
        assert_eq!(sqrt.descriptor_owner_tag(), None);

        let getitem =
            lookup_metadata("list.__getitem__").expect("list.__getitem__ must be registered");
        assert_eq!(getitem.kind, BuiltinCallableKind::MethodDescriptor);
        assert_eq!(getitem.python_name(), "__getitem__");
        assert_eq!(getitem.python_qualname(), "list.__getitem__");
        assert_eq!(getitem.python_module(), None);
        assert_eq!(getitem.owner(), Some("list"));
        assert_eq!(
            getitem.descriptor_owner_tag(),
            Some(pyrust_core::CanonicalClassTag::List)
        );
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
    fn module_generation_owns_native_callable_objects() {
        // The derive layer owns the lifetime policy: ordinary native modules
        // get one callable object per generated module, while `@flat
        // builtins` intentionally reuses the process/thread-wide objects
        // installed in the global namespace. The registry dispatch name
        // remains shared in both cases.
        let math_a =
            crate::builtin_modules::load_builtin_module("math").expect("first math module");
        let math_b =
            crate::builtin_modules::load_builtin_module("math").expect("second math module");
        let sqrt_a = module_attr(&math_a, "sqrt");
        let sqrt_b = module_attr(&math_b, "sqrt");
        assert_ne!(sqrt_a, sqrt_b, "math generations must not share sqrt");
        assert_ne!(sqrt_a.value_id(), sqrt_b.value_id());
        let (ValueKind::BuiltinFunction(name_a), ValueKind::BuiltinFunction(name_b)) =
            (sqrt_a.kind(), sqrt_b.kind())
        else {
            panic!("math.sqrt must be a BuiltinFunction");
        };
        assert!(
            std::ptr::eq(name_a, name_b),
            "fresh objects must retain the interned registry dispatch name"
        );

        let itertools_a =
            crate::builtin_modules::load_builtin_module("itertools").expect("first itertools");
        let itertools_b =
            crate::builtin_modules::load_builtin_module("itertools").expect("second itertools");
        let chain_a = module_attr(&itertools_a, "chain");
        let chain_b = module_attr(&itertools_b, "chain");
        let descriptor = |class: &Value| {
            let ValueKind::PyClass(class) = class.kind() else {
                panic!("itertools.chain must be a PyClass");
            };
            class
                .borrow()
                .attrs
                .get("__next__")
                .expect("chain.__next__ descriptor")
                .clone()
        };
        let next_a = descriptor(&chain_a);
        let next_b = descriptor(&chain_b);
        assert_ne!(
            next_a, next_b,
            "native class descriptors must be generation-local"
        );
        assert_ne!(next_a.value_id(), next_b.value_id());

        let builtins_a =
            crate::builtin_modules::load_builtin_module("builtins").expect("first builtins");
        let builtins_b =
            crate::builtin_modules::load_builtin_module("builtins").expect("second builtins");
        let len_a = module_attr(&builtins_a, "len");
        let len_b = module_attr(&builtins_b, "len");
        assert_eq!(len_a, len_b, "flat builtins must retain shared identity");
        assert_eq!(len_a.value_id(), len_b.value_id());
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

    #[test]
    fn lookup_name_returns_interned_static_str() {
        // `lookup_name` must return the exact `&'static str` pointer stored
        // in the registry, not a freshly allocated copy.  This pins the
        // contract that `Value::builtin_function(lookup_name(n).unwrap())`
        // reuses the single interned string rather than leaking a new Box.
        let name = lookup_name("abs").expect("abs must be registered");
        assert_eq!(name, "abs");
        // The returned pointer must be the one stored in the registry entry.
        let reg_name = REGISTRY
            .iter()
            .find(|r| r.name == "abs")
            .expect("abs in registry")
            .name;
        assert!(
            std::ptr::eq(name, reg_name),
            "lookup_name must return the registry's own &'static str pointer"
        );
    }

    #[test]
    fn every_flat_namespace_registry_entry_has_a_name() {
        // Acceptance criterion from issue #440: every flat-namespace builtin
        // registered in REGISTRY must be reachable via `lookup_name`.  Since
        // `resolve_builtin` in `expr.rs` delegates the non-primitive, non-
        // NotImplemented path entirely to `lookup_name`, this test guarantees
        // that a new `fn foo(…)` added to a `@flat pyrust_module!` body
        // becomes accessible as a bare global with no edits to `expr.rs`.
        for reg in REGISTRY.iter() {
            // Module-namespaced names (`math.sqrt`, …) are not bare-global
            // candidates; skip them.
            if reg.name.contains('.') {
                continue;
            }
            let found = lookup_name(reg.name);
            assert!(
                found.is_some(),
                "registered flat builtin {:?} must be found by lookup_name",
                reg.name
            );
            assert_eq!(
                found.unwrap(),
                reg.name,
                "lookup_name must return the same string it was queried with"
            );
        }
    }
}
