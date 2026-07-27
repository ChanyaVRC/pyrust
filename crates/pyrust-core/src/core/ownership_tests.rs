//! Compile-time-adjacent guards for the crate's ownership boundaries.

#[test]
fn crate_root_is_an_explicit_facade() {
    let root = include_str!("../lib.rs");
    assert!(
        !root.contains("include!("),
        "lib.rs must declare real Rust modules, not merge implementation files"
    );
    assert!(
        !root
            .lines()
            .any(|line| line.trim_start().starts_with("pub use ") && line.contains("::*")),
        "the public facade must enumerate exports explicitly"
    );
}

#[test]
fn production_domains_do_not_erase_module_boundaries() {
    fn visit(path: &std::path::Path) {
        for entry in std::fs::read_dir(path).expect("read pyrust-core source directory") {
            let path = entry.expect("read pyrust-core source entry").path();
            if path.is_dir() {
                visit(&path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs")
                || path.file_name().and_then(|name| name.to_str()) == Some("ownership_tests.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read pyrust-core Rust source");
            assert!(
                !source.contains("use super::*"),
                "{} must declare its dependencies explicitly",
                path.display()
            );
        }
    }

    visit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}

#[test]
fn non_value_responsibilities_are_not_folded_back_into_object_model() {
    let source = include_str!("object_model.rs");
    for forbidden in [
        "include!(\"arguments.rs\")",
        "include!(\"environment.rs\")",
        "include!(\"errors.rs\")",
        "include!(\"traceback.rs\")",
        "include!(\"class_epoch.rs\")",
        "include!(\"object_identity.rs\")",
        "include!(\"string_interning.rs\")",
        "include!(\"cycle_guards.rs\")",
    ] {
        assert!(
            !source.contains(forbidden),
            "object_model must not absorb {forbidden}"
        );
    }
}

#[test]
fn filesystem_module_namespace_does_not_strongly_own_its_environment() {
    let modules = include_str!("instance_attrs/module.rs");
    assert!(
        modules.contains("environment: Weak<RefCell<Environment>>"),
        "filesystem PyModule state must keep a weak environment link"
    );
    assert!(
        !modules.contains("environment: EnvRef,"),
        "a strong PyModule -> Environment edge leaks circular imports"
    );
}

#[test]
fn core_does_not_classify_python_builtin_callable_names() {
    let source = include_str!("value_helpers/descriptors.rs");
    for forbidden in [
        "rsplit_once",
        "split_once",
        "\"list\"",
        "\"dict\"",
        "\"object\"",
        "\"__len__\"",
        "\"__getitem__\"",
    ] {
        assert!(
            !source.contains(forbidden),
            "builtin callable API knowledge belongs to the installed provider: {forbidden}"
        );
    }
}

#[test]
fn core_exception_display_uses_typed_builtin_identity() {
    let source = include_str!("value_helpers/exception_display.rs");
    for forbidden in ["\"bytearray\"", "\"frozenset\""] {
        assert!(
            !source.contains(forbidden),
            "exception display must not infer primitive identity from {forbidden}"
        );
    }
    assert!(
        source.contains("ops.canonical_class_tag()"),
        "exception display must consume BuiltinTypeOps::canonical_class_tag"
    );
}

#[test]
fn fresh_nanbox_allocations_use_the_checked_helper() {
    for (path, source) in [
        ("value_model.rs", include_str!("value_model.rs")),
        (
            "value_constructors.rs",
            include_str!("value_constructors.rs"),
        ),
    ] {
        for raw_allocation in ["unsafe { alloc(", "let ptr = alloc("] {
            assert!(
                !source.contains(raw_allocation),
                "{path} performs a fresh allocation without alloc_or_handle"
            );
        }
    }
}

#[test]
fn nanboxed_value_stays_thread_bound_without_growing() {
    let model = include_str!("value_model.rs");
    assert!(
        model.contains("type ThreadBoundValueMarker = std::marker::PhantomData<Rc<()>>;"),
        "Value must retain a zero-sized !Send/!Sync ownership marker"
    );
    assert!(
        model.contains("pub struct Value(u64, ThreadBoundValueMarker);"),
        "Value must carry the thread-bound marker in its transparent representation"
    );
    for required_compile_fail_guard in ["fn require_send<T: Send>()", "fn require_sync<T: Sync>()"]
    {
        assert!(
            model.contains(required_compile_fail_guard),
            "Value docs must retain the {required_compile_fail_guard} compile-fail guard"
        );
    }

    for (path, source) in [
        (
            "value_constructors.rs",
            include_str!("value_constructors.rs"),
        ),
        ("value_lifecycle.rs", include_str!("value_lifecycle.rs")),
        ("nanbox_strings.rs", include_str!("nanbox_strings.rs")),
    ] {
        assert!(
            !source.contains("Value("),
            "{path} bypasses Value::from_bits and can silently omit the thread-bound marker"
        );
    }
}

struct FirstTestOps;
struct SecondTestOps;

impl crate::BuiltinTypeOps for FirstTestOps {
    fn type_name(&self) -> &'static str {
        "same-presentation-name"
    }
}

impl crate::BuiltinTypeOps for SecondTestOps {
    fn type_name(&self) -> &'static str {
        "same-presentation-name"
    }
}

#[test]
fn builtin_ops_identity_is_concrete_type_based() {
    let first: &dyn crate::BuiltinTypeOps = &FirstTestOps;
    let second: &dyn crate::BuiltinTypeOps = &SecondTestOps;

    assert!(crate::builtin_ops_is::<FirstTestOps>(first));
    assert!(!crate::builtin_ops_is::<FirstTestOps>(second));
    assert!(crate::builtin_ops_is::<SecondTestOps>(second));
}
