use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

struct SourceFile {
    path: PathBuf,
    contents: String,
}

/// Load an ownership domain exactly as Rust composes it: start from each
/// entry point and recursively follow every literal `include!("*.rs")`.
///
/// Keeping this dynamic avoids the old failure mode where a new fragment
/// was added to (for example) `builtin_methods.rs` but silently omitted
/// from the hand-maintained `concat!(include_str!(...))` test fixture.
fn include_graph(entry_points: &[&str]) -> Vec<SourceFile> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter");
    let mut pending: Vec<PathBuf> = entry_points
        .iter()
        .map(|entry| source_root.join(entry))
        .collect();
    let mut visited = BTreeSet::new();
    let mut sources = Vec::new();

    while let Some(path) = pending.pop() {
        let path = path
            .canonicalize()
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
        if !visited.insert(path.clone()) {
            continue;
        }
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
        let parent = path.parent().expect("Rust source must have a parent");
        for line in contents.lines() {
            let Some(rest) = line.trim().strip_prefix("include!(\"") else {
                continue;
            };
            let Some(relative) = rest.strip_suffix("\");") else {
                continue;
            };
            let included = parent.join(relative);
            assert_eq!(
                included
                    .extension()
                    .and_then(|extension| extension.to_str()),
                Some("rs"),
                "{} includes a non-Rust source through include!(); use include_str!() for data",
                path.display()
            );
            pending.push(included);
        }
        sources.push(SourceFile { path, contents });
    }
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    sources
}

fn assert_domain_excludes(layer: &str, sources: &[SourceFile], forbidden: &str, guidance: &str) {
    for source in sources {
        assert!(
            !source.contents.contains(forbidden),
            "{layer} source {} contains {forbidden}; {guidance}",
            source.path.display()
        );
    }
}

#[test]
fn interpreter_does_not_own_root_namespace_cache_or_globals_state() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter.rs");
    let interpreter = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
    for forbidden_field in [
        "pub(crate) module_globals_dict:",
        "pub(crate) globals_accessed:",
        "pub(crate) global_env_version:",
        "pub(crate) global_struct_version:",
    ] {
        assert!(
            !interpreter.contains(forbidden_field),
            "root namespace state must live on Environment, not Interpreter: {forbidden_field}"
        );
    }
}

#[test]
fn replaceable_module_class_cache_is_not_a_python_object_owner() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter.rs");
    let interpreter = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
    assert!(
        interpreter.contains("class: Weak<RefCell<PyClass>>"),
        "a module-class cache must not strongly retain a replaced module generation"
    );
}

// These are concrete Python APIs, not data-model protocols. Their exact
// names belong to builtin_methods or generator_protocols.
const CONCRETE_API_NAMES: &[&str] = &[
    "id",
    "property",
    "classmethod",
    "staticmethod",
    "append",
    "extend",
    "insert",
    "remove",
    "pop",
    "clear",
    "index",
    "count",
    "sort",
    "reverse",
    "copy",
    "update",
    "setdefault",
    "fromkeys",
    "get",
    "keys",
    "values",
    "items",
    "add",
    "discard",
    "difference",
    "intersection",
    "union",
    "join",
    "split",
    "replace",
    "startswith",
    "endswith",
    "strip",
    "upper",
    "lower",
    "send",
    "throw",
    "close",
    "asend",
    "athrow",
    "aclose",
    "getter",
    "setter",
    "deleter",
    "typing",
    "NamedTuple",
    "TypedDict",
    "_namedtuple_functional",
    "_typeddict_functional",
];

#[test]
fn runtime_facade_keeps_ownership_tests_out_of_its_domain_declarations() {
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter/runtime.rs");
    let runtime = std::fs::read_to_string(runtime_path).expect("runtime facade must be readable");
    assert!(
        runtime.contains("include!(\"runtime/ownership_tests.rs\");"),
        "runtime ownership tests must stay in their dedicated source"
    );
    assert!(
        !runtime.contains("fn include_graph("),
        "test infrastructure must not be folded back into the runtime facade"
    );
}

#[test]
fn exception_groups_stay_separate_from_generic_exception_control() {
    let control = include_graph(&["runtime/exceptions/control.rs"]);
    assert_domain_excludes(
        "generic exception control",
        &control,
        "fn split_exception_group(",
        "keep PEP 654 matching and derivation in exceptions/groups.rs",
    );
    let groups = include_graph(&["runtime/exceptions/groups.rs"]);
    assert!(
        groups
            .iter()
            .any(|source| source.contents.contains("fn split_exception_group(")),
        "exceptions/groups.rs must own PEP 654 matching"
    );
}

#[test]
fn exception_slot_schema_stays_in_exception_domain() {
    let slots = include_graph(&["runtime/exceptions/slots.rs"]);
    assert!(
        slots.iter().any(|source| {
            source.contents.contains("enum ExceptionSlotPolicy")
                && source.contents.contains("fn exception_slot_policy(")
        }),
        "exceptions/slots.rs must own the typed native exception-slot schema"
    );

    let attributes = include_graph(&["runtime/attributes.rs"]);
    for concrete_slot in [
        "\"characters_written\"",
        "\"print_file_and_line\"",
        "\"filename2\"",
    ] {
        assert_domain_excludes(
            "generic attribute policy",
            &attributes,
            concrete_slot,
            "consume ExceptionSlotPolicy instead of relisting exception-family fields",
        );
    }

    let proxy_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("pyrust crate must have a workspace crates directory")
        .join("pyrust-builtins/src/instance_dict.rs");
    let proxy = std::fs::read_to_string(&proxy_path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", proxy_path.display()));
    assert!(
        !proxy.contains("fn is_exception_slot_for_class(")
            && !proxy.contains("fn exception_slot_policy("),
        "instance_dict must remain a proxy/storage owner, not an exception-slot schema"
    );
}

#[test]
fn primitive_slot_metadata_stays_separate_from_object_protocol_behaviour() {
    let object_protocol = include_graph(&["runtime/builtin_methods/object_protocol.rs"]);
    assert_domain_excludes(
        "object protocol behaviour",
        &object_protocol,
        "fn slot_dunder_table(",
        "keep primitive slot metadata in builtin_methods/slot_tables.rs",
    );
    let slot_tables = include_graph(&["runtime/builtin_methods/slot_tables.rs"]);
    assert!(
        slot_tables
            .iter()
            .any(|source| source.contents.contains("fn slot_dunder_table(")),
        "builtin_methods/slot_tables.rs must own primitive slot metadata"
    );
}

#[test]
fn primitive_class_bootstrap_consumes_provider_owned_surfaces() {
    let primitive_classes = include_graph(&["helpers/primitive_classes.rs"]);
    for concrete_api in [
        "\"fromhex\"",
        "\"from_bytes\"",
        "\"fromkeys\"",
        "\"maketrans\"",
        "const INT_METHODS",
        "const STR_METHODS",
        "const LIST_METHODS",
        "const DICT_METHODS",
    ] {
        assert_domain_excludes(
            "primitive class bootstrap",
            &primitive_classes,
            concrete_api,
            "declare the class surface and descriptor category in the owning pyrust-builtins module",
        );
    }
    assert!(
        primitive_classes
            .iter()
            .any(|source| source.contents.contains("PrimitiveClassAttrs")),
        "primitive class bootstrap must consume typed provider metadata"
    );
}

#[test]
fn runtime_domains_declare_parent_dependencies_explicitly() {
    let wildcard_parent_import = concat!("use super::", "*");
    let runtime = include_graph(&["runtime.rs"]);
    assert_domain_excludes(
        "runtime domain",
        &runtime,
        wildcard_parent_import,
        "list its actual dependencies at the domain boundary",
    );
}

#[test]
fn foundational_routers_do_not_own_concrete_python_api_names() {
    for (layer, sources) in [
        (
            "call routing",
            include_graph(&["runtime/call_dispatch.rs", "runtime/calls.rs"]),
        ),
        (
            "VM execution",
            include_graph(&["runtime/vm.rs", "runtime/fast_path.rs"]),
        ),
        (
            "generic attribute routing",
            include_graph(&["runtime/attributes.rs"]),
        ),
    ] {
        for name in CONCRETE_API_NAMES {
            let literal = format!("\"{name}\"");
            assert_domain_excludes(
                layer,
                &sources,
                &literal,
                "move that policy to builtin_methods or its protocol owner",
            );
        }
    }
}

#[test]
fn foundational_value_classification_does_not_decode_builtin_display_names() {
    let classification = include_graph(&[
        "helpers/builtin_layout.rs",
        "helpers/coercion.rs",
        "helpers/numeric_compare.rs",
        "helpers/primitive_classes.rs",
        "helpers/value_identity.rs",
        "runtime/collection_keys/hashing.rs",
        "runtime/collection_keys/key_conversion.rs",
        "runtime/type_objects/value_class.rs",
        "runtime/expr/complex_union.rs",
        "runtime/expr/indexing.rs",
        "runtime/iteration/iterator_factory.rs",
        "runtime/collection_ops/mapping.rs",
        "runtime/slicing.rs",
        "runtime/builtin_methods/attribute_adapters.rs",
        "runtime/builtin_methods/bound_objects.rs",
        "runtime/builtin_methods/object_protocol.rs",
        "runtime/attributes/attribute_assignment.rs",
        "runtime/attributes/instance_attributes.rs",
        "runtime/introspection/tracebacks.rs",
    ]);
    for implementation in [
        "ops.type_name() ==",
        "ops.type_name() !=",
        "match ops.type_name()",
        "let name = ops.type_name()",
    ] {
        assert_domain_excludes(
            "foundational value classification",
            &classification,
            implementation,
            "consume a canonical tag or a typed predicate from the owning builtin module",
        );
    }
}

#[test]
fn generic_binding_does_not_classify_builtin_registry_names() {
    let generic_binding = include_graph(&["runtime/attributes.rs", "helpers/call_protocol.rs"]);
    for implementation in [
        "is_builtin_classmethod",
        "rfind('.')",
        "split_once('.')",
        "\"object.__init_subclass__\"",
        "\"type.__prepare__\"",
        "\"collections.abc.__instancecheck__\"",
        "\"pathlib.Path.cwd\"",
    ] {
        assert_domain_excludes(
            "generic descriptor binding",
            &generic_binding,
            implementation,
            "install an explicit descriptor in the owning class or module",
        );
    }
}

#[test]
fn generic_call_runtime_does_not_decode_builtin_representations() {
    let call_routing = include_graph(&["runtime/call_dispatch.rs", "runtime/calls.rs"]);
    for implementation in [
        "pyrust_builtins::",
        "crate::builtin_modules::",
        "load_module(\"typing\")",
        "ValueKind::BuiltinFunction(\"",
    ] {
        assert_domain_excludes(
            "generic call runtime",
            &call_routing,
            implementation,
            "delegate through builtin_methods or a typed service",
        );
    }
}

#[test]
fn execution_and_calls_do_not_decode_exception_registry_keys() {
    let foundational_runtime = include_graph(&[
        "runtime/call_dispatch.rs",
        "runtime/calls.rs",
        "runtime/vm.rs",
    ]);
    for implementation in ["exc_classes.", "instantiate_named_exception("] {
        assert_domain_excludes(
            "execution and call routing",
            &foundational_runtime,
            implementation,
            "request a typed exception result from the exceptions domain",
        );
    }
}

#[test]
fn execution_and_expression_domains_do_not_borrow_register_values() {
    for (layer, sources) in [
        ("VM execution", include_graph(&["runtime/vm.rs"])),
        ("expression evaluation", include_graph(&["runtime/expr.rs"])),
    ] {
        assert_domain_excludes(
            layer,
            &sources,
            "fn vm_read_ref",
            "register reads must return an owned O(1) Value clone so borrows never cross \
                 interpreter re-entry",
        );
    }
}

#[test]
fn vm_opcode_loop_does_not_own_concrete_iteration_adapters() {
    let vm_opcode_loop = include_graph(&["runtime/vm/execute.rs"]);
    for adapter in [
        "pyrust_builtins::bytearray",
        "pyrust_builtins::dict_views",
        "dict_subclass_iter_semantics",
        "effective_user_iter",
        "is_ordered_dict_class_or_subclass",
        "downcast_ref::<GetItemIter>",
        "downcast_ref::<CallableIter>",
        "downcast_ref::<MapIter>",
        "downcast_ref::<FilterIter>",
        "downcast_ref::<ZipIter>",
        "downcast_ref::<BigRangeIter>",
        "downcast_mut::<NativeIterFrame>",
        "lookup_class_attr(&class, \"__next__\")",
    ] {
        assert_domain_excludes(
            "VM opcode loop",
            &vm_opcode_loop,
            adapter,
            "move iterator-state policy to the iteration domain",
        );
    }
}

#[test]
fn vm_state_does_not_own_iterator_models() {
    let vm_state = include_graph(&["runtime/vm/state.rs"]);
    for model in [
        "pub(crate) enum IterState",
        "pub(crate) struct NativeIterFrame",
        "impl NativeIterFrame",
        "pub(crate) struct GetItemIter",
        "pub(crate) struct CallableIter",
        "pub(crate) struct MapIter",
        "pub(crate) struct FilterIter",
        "pub(crate) struct ZipIter",
    ] {
        assert_domain_excludes("VM frame state", &vm_state, model, "move it to iteration");
    }
}

#[test]
fn generic_iteration_and_type_objects_do_not_own_stdlib_iterator_policy() {
    let generic_domains = include_graph(&["runtime/iteration.rs", "runtime/type_objects.rs"]);
    for provider_detail in [
        "\"itertools.",
        "\"OrderedDict",
        "\"odict_iterator\"",
        "ChainFromIterableIter",
        "TeeSharedState",
        "TeeIter",
        "ORDERED_DICT_CLASSES",
        "OD_CLEAR_REGISTRY",
    ] {
        assert_domain_excludes(
            "generic iteration/type-object",
            &generic_domains,
            provider_detail,
            "keep standard-library cursor state, names, registries, and diagnostics behind \
             ProviderIterator or a typed pyrust-builtins identity policy",
        );
    }
}

#[test]
fn generic_class_mutation_uses_typed_stdlib_immutability_policy() {
    let mutation_paths = include_graph(&[
        "runtime/attributes/attribute_assignment.rs",
        "runtime/attributes/attribute_deletion.rs",
    ]);
    for provider_detail in ["\"collections.OrderedDict\"", "is_ordered_dict_class("] {
        assert_domain_excludes(
            "generic class attribute mutation",
            &mutation_paths,
            provider_detail,
            "request the provider's typed immutable-class diagnostic",
        );
    }
}

#[test]
fn vm_opcode_loop_delegates_cache_implementations() {
    let vm_opcode_loop = include_graph(&["runtime/vm/execute.rs"]);
    for implementation in [
        "call_builtin_cache",
        "CallBuiltinCacheEntry",
        "builtin_registry::",
        "memo_cache",
        "memo_stats",
        "int_int_fast",
        "num_binop_fast",
        "try_str_inplace_concat",
        "IterState::MaterializedGuarded",
        "IterState::ItemsGuarded",
        "IterState::Range",
        "ends_with(\"_getframe\")",
        "pyrust_builtins::",
        "value_to_pykey",
        "set_lookup",
        "dict_insert",
        "dict_extend_value_dedup",
        "collect_iterable",
        "module_globals_dict",
        "global_cache_interest_masks",
        "resolve_builtin",
        "fn materialize_pyerror",
        "caught_traceback_value(",
        "build_deferred_traceback(",
        "attrs.get_mut(\"__traceback__\")",
        "get_cloned_or_slot(\"__traceback__\")",
        ".insert(\"__cause__\"",
        ".insert(\"__context__\"",
        ".insert(\"__suppress_context__\"",
    ] {
        assert_domain_excludes(
            "VM opcode loop",
            &vm_opcode_loop,
            implementation,
            "delegate it to fast_path or its protocol owner",
        );
    }
}

#[test]
fn fast_paths_consume_typed_runtime_services() {
    let fast_paths = include_graph(&["runtime/fast_path.rs"]);
    for representation in [
        "pyrust_builtins::",
        "crate::builtin_modules::",
        "rfind('.')",
        "split_once('.')",
        "instance_builtin_data",
        "__builtin_data__",
        "lookup_class_attr",
        "fn expand_kwargs_into",
        "mapping_entries_for_expansion",
        "descriptor_cache_kind",
        "is_typevar_class",
        "is_data_descriptor",
        "class_uses_default_setattr",
        "object.__setattr__",
        "fn eval_builtin_unary",
    ] {
        assert_domain_excludes(
            "fast path",
            &fast_paths,
            representation,
            "expose a typed decision from the semantic owner",
        );
    }
}

#[test]
fn execution_and_fast_path_are_distinct_rust_modules() {
    let runtime_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter/runtime.rs");
    let runtime = std::fs::read_to_string(&runtime_path).expect("runtime facade must be readable");
    assert!(
        runtime.contains("mod fast_path {"),
        "fast_path must be a real sibling Rust module"
    );
    let execution_body = runtime
        .split_once("mod execution {")
        .and_then(|(_, remainder)| remainder.split_once("\nmod generator_protocols {"))
        .map(|(body, _)| body)
        .expect("execution module must precede generator_protocols");
    assert!(
        !execution_body.contains("runtime/fast_path"),
        "execution and fast_path must not share one include scope"
    );
}

#[test]
fn call_method_cache_and_opcode_adapter_live_in_fast_path() {
    let builtin_methods = include_graph(&["runtime/builtin_methods.rs"]);
    for implementation in [
        "AttrCacheEntry",
        "call_site_pc",
        "name_idx",
        "args_base",
        "fn exec_call_method(",
        "fn resolve_method_cached(",
    ] {
        assert_domain_excludes(
            "builtin method semantics",
            &builtin_methods,
            implementation,
            "move bytecode cache and opcode adaptation to fast_path",
        );
    }

    let fast_paths = include_graph(&["runtime/fast_path.rs"]);
    for expected in ["fast_path/method_cache.rs", "fast_path/method_call.rs"] {
        assert!(
            fast_paths
                .iter()
                .any(|source| source.path.ends_with(expected)),
            "fast_path include graph must own {expected}"
        );
    }
}

#[test]
fn call_method_fast_path_does_not_decode_builtin_names_or_backing() {
    let method_fast_path = include_graph(&[
        "runtime/fast_path/method_cache.rs",
        "runtime/fast_path/method_call.rs",
    ]);
    for representation in [
        "instance_builtin_data",
        "__builtin_data__",
        "lookup_class_attr",
        "__getattribute__",
        "\"dict\"",
        "\"list\"",
        "\"set\"",
    ] {
        assert_domain_excludes(
            "CallMethod fast path",
            &method_fast_path,
            representation,
            "delegate exact builtin policy through a typed builtin_methods service",
        );
    }
}

#[test]
fn builtin_methods_do_not_own_generic_iteration_semantics() {
    let builtin_methods = include_graph(&["runtime/builtin_methods.rs"]);
    for service in [
        "fn collect_iterable",
        "fn call_next",
        "fn make_getitem_iter",
        "fn step_getitem_iter",
        "fn step_map_iter",
        "fn step_filter_iter",
        "fn step_zip_iter",
    ] {
        assert_domain_excludes(
            "builtin method layer",
            &builtin_methods,
            service,
            "move it to the iteration domain",
        );
    }
}

#[test]
fn builtin_methods_do_not_own_value_formatting_policy() {
    let builtin_methods = include_graph(&["runtime/builtin_methods.rs"]);
    for service in [
        "fn render_value_as_str",
        "fn render_value_repr",
        "fn render_key_repr",
    ] {
        assert_domain_excludes(
            "builtin method layer",
            &builtin_methods,
            service,
            "move renderer policy to the formatting domain and consume it as a typed service",
        );
    }
}

#[test]
fn formatting_does_not_own_concrete_string_method_signatures() {
    let formatting = include_graph(&["runtime/formatting.rs"]);
    for name in ["split", "rsplit", "splitlines", "encode", "expandtabs"] {
        let literal = format!("\"{name}\"");
        assert_domain_excludes(
            "formatting",
            &formatting,
            &literal,
            "move concrete str method binding to builtin_methods",
        );
    }
}

#[test]
fn splitlines_truth_protocol_is_owned_by_builtin_method_adapters() {
    let builtin_methods = include_graph(&["runtime/builtin_methods.rs"]);
    let text_bytes = builtin_methods
        .iter()
        .find(|source| {
            source
                .path
                .file_name()
                .is_some_and(|name| name == "text_bytes.rs")
        })
        .expect("builtin_methods must include its text/bytes protocol adapter");

    assert!(
        text_bytes
            .contents
            .contains("fn truthify_splitlines_keepends"),
        "splitlines truth normalization must have one builtin-method boundary"
    );
    assert!(
        text_bytes.contents.contains("self.truthy_value(value)?"),
        "non-primitive keepends values must use the canonical truth protocol"
    );
    assert!(
        text_bytes
            .contents
            .contains("fn bind_bytearray_splitlines_keepends"),
        "bytearray dispatch must share the splitlines truth adapter"
    );

    for source in &builtin_methods {
        let file_name = source
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if file_name != "text_bytes.rs" {
            for forbidden in [
                "pyrust_builtins::string::call(",
                "pyrust_builtins::string::call_prevalidated(",
                "pyrust_builtins::bytes::call(",
                "pyrust_builtins::bytes::call_prevalidated(",
                "call_bytes_method_coerced_prevalidated(",
            ] {
                assert!(
                    !source.contents.contains(forbidden),
                    "{} bypasses the interpreter-aware text/bytes adapter via {forbidden}",
                    source.path.display()
                );
            }
        }
    }

    for file_name in ["bound_objects.rs", "bound_instances.rs", "unbound.rs"] {
        let source = builtin_methods
            .iter()
            .find(|source| {
                source
                    .path
                    .file_name()
                    .is_some_and(|name| name == file_name)
            })
            .unwrap_or_else(|| panic!("builtin_methods must include {file_name}"));
        assert!(
            source
                .contents
                .contains("bind_bytearray_splitlines_keepends"),
            "{file_name} must normalize bytearray.splitlines before BuiltinTypeOps dispatch"
        );
    }
}

#[test]
fn generic_attribute_routing_does_not_own_object_builtin_api_names() {
    let attributes = include_graph(&["runtime/attributes.rs"]);
    for name in [
        "__sizeof__",
        "__dir__",
        "__reduce__",
        "__reduce_ex__",
        "__getstate__",
    ] {
        let literal = format!("\"{name}\"");
        assert_domain_excludes(
            "generic attribute routing",
            &attributes,
            &literal,
            "move exact object built-in API policy to builtin_methods",
        );
    }
}

#[test]
fn class_construction_uses_protocols_and_builtin_extension_hooks() {
    let class_construction = include_graph(&["runtime/classes.rs"]);
    for implementation in [
        "is_namedtuple_marker",
        "is_typeddict_marker",
        "_build_namedtuple_class",
        "_build_typeddict_class",
        "as_generic_alias_origin",
        "load_module(\"typing\")",
        "pyrust_builtins::",
    ] {
        assert_domain_excludes(
            "generic class construction",
            &class_construction,
            implementation,
            "use a data-model protocol or builtin adapter",
        );
    }
}

fn rust_sources_below(root: &std::path::Path, sources: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(root).expect("source directory must be readable") {
        let path = entry.expect("source entry must be readable").path();
        if path.is_dir() {
            rust_sources_below(&path, sources);
        } else if path
            .file_name()
            .is_some_and(|name| name == "ownership_tests.rs")
        {
            // This scanner guards production ownership. Its own string
            // fixtures deliberately mention the forbidden dependency names.
            continue;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
}

#[test]
fn runtime_does_not_depend_on_flat_builtins_implementation() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter/runtime");
    let mut sources = Vec::new();
    rust_sources_below(&root, &mut sources);
    let forbidden = concat!("crate::builtin_modules::", "builtins");
    for path in sources {
        let source = std::fs::read_to_string(&path).expect("runtime source must be readable");
        assert!(
            !source.contains(forbidden),
            "{} depends on the flat builtins implementation; expose a typed runtime service \
                 and make builtins consume it instead",
            path.display()
        );
    }
}

#[test]
fn runtime_only_uses_the_builtin_module_provider_at_the_import_boundary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter/runtime");
    let import_boundary = root.join("namespaces/modules.rs");
    let mut sources = Vec::new();
    rust_sources_below(&root, &mut sources);
    let provider_prefix = "crate::builtin_modules::";
    let allowed_calls = [
        "crate::builtin_modules::load_builtin_module",
        "crate::builtin_modules::prepare_builtin_module",
        "crate::builtin_modules::post_load_inject",
    ];
    for path in sources {
        let source = std::fs::read_to_string(&path).expect("runtime source must be readable");
        for line in source.lines().filter(|line| line.contains(provider_prefix)) {
            assert_eq!(
                path,
                import_boundary,
                "{} reaches into the built-in module provider outside the import boundary",
                path.display()
            );
            assert!(
                allowed_calls.iter().any(|allowed| line.contains(allowed)),
                "{} reaches a concrete built-in module implementation: {line}",
                path.display()
            );
        }
    }
}

#[test]
fn shared_namespace_compiler_stays_on_the_filesystem_import_path() {
    let runtime_root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter/runtime");
    let expected_owner = runtime_root.join("program_execution/script.rs");
    let constructor = "compile_shared_namespace_module_with_linenos(";
    let mut sources = Vec::new();
    rust_sources_below(&runtime_root, &mut sources);
    let users: Vec<_> = sources
        .into_iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .expect("runtime source must be readable")
                .contains(constructor)
        })
        .collect();

    assert_eq!(
        users,
        vec![expected_owner],
        "shared module-global compilation must remain limited to filesystem import execution; \
         ordinary scripts and exec() retain their fast-local compiler path"
    );
}

#[test]
fn builtins_does_not_own_generic_runtime_protocol_services() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/builtin_modules/bodies/builtins");
    let mut sources = vec![
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/builtin_modules/bodies/builtins.rs"),
    ];
    rust_sources_below(&root, &mut sources);
    let forbidden_definitions = [
        "fn hash_value_with_interp",
        "fn value_needs_interp",
        "fn make_iterator",
        "fn value_class",
        "fn is_coroutine_value",
        "fn is_async_generator_value",
        "fn render_value_repr",
        "fn render_key_repr",
        "fn full_type_name_str",
        "fn from_bytes_source_to_bytes",
    ];
    for path in sources {
        let source = std::fs::read_to_string(&path).expect("builtins source must be readable");
        for definition in forbidden_definitions {
            assert!(
                !source.contains(definition),
                "{} defines generic runtime service {definition}; move it to its runtime domain",
                path.display()
            );
        }
    }
}

#[test]
fn round_ndigits_uses_the_shared_index_protocol() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/builtin_modules/bodies/builtins/rounding.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
    assert!(
        source.contains(".value_to_index("),
        "round's ndigits clamp must consume the runtime index protocol"
    );
    assert!(
        !source.contains("\"__index__\""),
        "round must not duplicate __index__ lookup or result validation"
    );
}

#[test]
fn numeric_consumers_do_not_duplicate_index_slot_resolution() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for relative in [
        "builtin_modules/bodies/builtins/int_constructor.rs",
        "builtin_modules/bodies/builtins/float_constructor.rs",
        "builtin_modules/bodies/builtins/complex_constructor.rs",
        "builtin_modules/bodies/builtins/parsing_bytes.rs",
        "builtin_modules/bodies/math/coercion.rs",
    ] {
        let path = root.join(relative);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
        assert!(
            !source.contains("\"__index__\""),
            "{} must consume the shared index protocol instead of looking up the slot",
            path.display(),
        );
    }

    // Integer printf intentionally retains a consumer-specific invalid-result
    // policy. Guard only the float-like path migrated in this change.
    let path = root.join("interpreter/runtime/formatting/printf_conversion.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
    let float_path = source
        .split_once("fn coerce_printf_float_arg")
        .and_then(|(_, tail)| tail.split_once("fn str_printf_convert"))
        .map(|(body, _)| body)
        .expect("printf float coercion function must remain discoverable");
    assert!(
        !float_path.contains("\"__index__\""),
        "float printf coercion must consume the shared index protocol"
    );
}

#[test]
fn builtin_modules_do_not_own_runtime_identity_or_import_services() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builtin_modules");
    let mut sources = Vec::new();
    rust_sources_below(&root, &mut sources);
    let forbidden_definitions = [
        "fn import_module_registry",
        "fn construct_mapping_proxy",
        "fn is_namedtuple_marker",
        "fn is_typeddict_marker",
        "fn generic_alias_origin",
        "static SYS_MODULES",
    ];
    for path in sources {
        let source =
            std::fs::read_to_string(&path).expect("built-in module source must be readable");
        for definition in forbidden_definitions {
            assert!(
                !source.contains(definition),
                "{} owns runtime service {definition}; move identity/construction to a \
                     runtime domain or use a Python data-model protocol",
                path.display()
            );
        }
    }
}
