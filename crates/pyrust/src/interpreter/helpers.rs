// Shared interpreter helpers are grouped by semantic domain. Keeping these as
// includes preserves the original module-private API while making ownership clear.

/// Resolve a dynamically selected built-in method to the registry's canonical
/// static dispatch key.
///
/// Primitive class singletons are rebuilt per interpreter thread, but their
/// method set is fixed at compile time. Reusing registry-owned names avoids
/// leaking one formatted string per method for every thread that touches those
/// singletons. Some legacy sentinel-only methods are intentionally absent from
/// the public built-in registry; those names are interned once per process as a
/// compatibility bridge until `Value::BuiltinFunction` owns a typed dispatch
/// key instead of requiring `&'static str`.
static LEGACY_BUILTIN_METHOD_NAMES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<&'static str>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

fn registered_builtin_method_name(owner: &str, method: &str) -> &'static str {
    let qualified = format!("{owner}.{method}");
    if let Some(name) = crate::builtin_registry::lookup_name(&qualified) {
        return name;
    }

    let mut names = LEGACY_BUILTIN_METHOD_NAMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(name) = names.get(qualified.as_str()) {
        return name;
    }

    let name: &'static str = Box::leak(qualified.into_boxed_str());
    names.insert(name);
    name
}

include!("helpers/numeric_compare.rs");
include!("helpers/class_lookup.rs");
include!("helpers/primitive_classes.rs");
include!("helpers/canonical_slots.rs");
include!("helpers/builtin_layout.rs");
include!("helpers/coercion.rs");
include!("helpers/call_protocol.rs");
include!("helpers/numeric_basics.rs");
include!("helpers/class_semantics.rs");
include!("helpers/exceptions.rs");
include!("helpers/builtins_provider.rs");
include!("helpers/builtins_and_locals.rs");
include!("helpers/name_resolution.rs");

mod scope_analysis {
    use super::{AssignTarget, Expr, HashMap, HashSet, Stmt};

    include!("helpers/scope_analysis/bindings.rs");
    include!("helpers/scope_analysis/declarations.rs");
    include!("helpers/scope_analysis/references.rs");
}
pub(crate) use scope_analysis::{
    check_global_nonlocal_order, collect_annotation_target_names, collect_global_names,
    collect_local_names, collect_nonlocal_names, compute_def_bound_mask,
};

include!("helpers/value_identity.rs");
include!("helpers/numeric_conversion.rs");
include!("helpers/purity.rs");
include!("helpers/numeric_math.rs");
include!("helpers/super_and_traceback.rs");
include!("helpers/tests.rs");
