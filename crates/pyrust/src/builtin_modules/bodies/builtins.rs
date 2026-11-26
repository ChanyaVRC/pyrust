// `builtins` module — included into `pub mod builtins { … }` declared by
// the `@flat builtins,` entry in `pyrust_builtin_modules!` in
// `builtin_modules/mod.rs`.
//
// `@flat` means functions register under their short name only (no
// `builtins.` prefix), so `abs` resolves to `BuiltinReg { name: "abs", … }`.
// Therefore `BuiltinFunction("abs")` from the global env (set up in
// `helpers.rs::register_builtins`) hits this dispatch via the registry
// probe in `runtime/builtin_methods::try_call_builtin_callable`. Importable as
// `import builtins` too, which yields a `PyModule { name: "builtins", … }`
// containing every fn declared here plus declared constants.
//
// Reference: <https://docs.python.org/3/library/functions.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytes, PyFloat, PyInt, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{
    BuiltinTypeClass, CallableIter, ConsumerIterator, EnumerateIter, FilterIter, Interpreter,
    IterSrcBuf, MapIter, NativeIteratorClass, ZipIter, apply_format_spec, apply_format_spec_named,
    ascii_repr_interp, bigint_divmod_floor, builtin_data_backing,
    builtin_type_class_isinstance_fast, class_has_native_builtin_type_ancestor,
    class_is_subclass_of, class_suppresses_instance_dict, classify_exception_class, coerce_numeric,
    coerce_subclass_backing, compare_values, compare_values_with_op, current_locals_value,
    dir_names, dispatch_numeric_binop, effective_builtin_receiver, extract_str_value,
    find_immutable_primitive_base, find_mutable_primitive_base, find_scalar_primitive_base,
    float_divmod, float_to_bigint, format_bin_i64, format_oct_i64, full_type_name_str,
    function_type_singleton, getitem_iterator_reduce, hash_value_with_interp,
    instance_builtin_data, invoke_class_method, is_getitem_iterator, is_reversed_iterator,
    is_str_or_str_subclass, iter_values, lookup_class_attr, lookup_value_special_method,
    make_iterator, make_reversed_dict_iter, make_reversed_getitem_iterator,
    make_reversed_mapping_snapshot_iter, make_reversed_range_iterator,
    make_reversed_sequence_iterator, mapping_pairs_via_protocol, method_type_singleton,
    modinv_bigint, modinv_i64, modpow_bigint, modpow_i64, native_iterator_class,
    native_iterator_reduce, normalize_complex_slot_result, normalize_float_slot_result,
    normalize_int_slot_result, primitive_class_by_name, primitive_class_dispatch, py_mod_i64,
    py_round_half_even_checked, reject_keyword_args_expanded, render_instance_str,
    render_value_repr, resolve_zero_arg_super, reversed_iterator_reduce, round_bigint_neg_ndigits,
    round_float_ndigits, sync_module_env_to_globals_dict, type_class_singleton,
    unicode_exc_set_attrs, value_class, value_to_float, value_type_name_str,
};
use crate::value::{
    InstanceAttrs, PyBigInt, PyClass, PyDict, PyKey, PySet, PyToPrimitive, PyZero, SortKind,
    UserFunctionKind, Value, ValueKind, classify_sort, range_len,
};

// Builtins are registered in independent semantic families.  `namespace.rs`
// walks them once to populate the single flat Python builtins namespace.

mod scalar {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyBool, PyBytes, PyError, PyInt, PyStr, PyValue,
        Value, ValueKind, chr_from_code_point, format_bigint_radix, format_bin_i64, format_hex_i64,
        format_index_radix, format_oct_i64, not_an_integer_err, value_type_name_str,
    };
    include!("builtins/scalar.rs");
}

mod identity_and_repr {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyValue, Result, Value, ValueKind,
        ascii_repr_interp, hash_value_with_interp, render_value_repr,
    };
    include!("builtins/identity_and_repr.rs");
}

mod numeric_functions {
    use super::{
        BinaryOp, ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyBigInt,
        PyBool, PyError, PyFloat, PyInt, PyValue, PyValueRef, PyZero, Rc, Result, Value, ValueKind,
        class_is_subclass_of, coerce_numeric, dispatch_numeric_binop, divmod_float_float,
        divmod_int_int, effective_builtin_receiver, invoke_class_method, lookup_class_attr,
        modinv_bigint, modinv_i64, modpow_bigint, modpow_i64, pyint_to_f64, value_to_float,
        value_type_name_str,
    };
    include!("builtins/numeric_functions.rs");
}

mod aggregation {
    use super::{
        BinaryOp, ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError,
        PyValue, PyValueRef, Result, Value, ValueKind,
    };
    include!("builtins/aggregation.rs");
}

mod iteration {
    use super::{
        BuiltinTypeClass, CallableIter, EnumerateIter, ExpandedCallArg, FN_PREFIX, FilterIter,
        Interpreter, IterSrcBuf, MODULE_NAME, MapIter, NativeIteratorClass, OptValueRef,
        OptionalArg, PyError, PyValue, PyValueRef, Rc, Result, Value, ValueKind, ZipIter,
        builtin_type_class_isinstance_fast, builtin_type_new, class_is_subclass_of,
        full_type_name_str, instance_builtin_data, invoke_class_method, iter_values,
        lookup_class_attr, make_iterator, make_reversed_dict_iter, make_reversed_getitem_iterator,
        make_reversed_mapping_snapshot_iter, make_reversed_range_iterator,
        make_reversed_sequence_iterator, reject_keyword_args_expanded, reversed_iterator_reduce,
        value_type_name_str,
    };
    include!("builtins/iteration.rs");
}

mod reflection {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyClass, PyError, PyKey,
        PyValue, PyValueRef, Rc, RefCell, Result, Value, ValueKind, attr_name_arg,
        calling_frame_has_future_annotations, class_suppresses_instance_dict, compare_values,
        current_locals_value, dir_names, extract_ast_linenos, extract_compile_filename,
        extract_source_text, inject_builtins_into_globals, instance_is_ast_subclass,
        invoke_class_method, is_ast_node, isinstance_check, issubclass_check, lookup_class_attr,
        reject_keyword_args_expanded, sync_module_env_to_globals_dict, value_class,
        value_type_name_str,
    };
    include!("builtins/reflection.rs");
}

mod length {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyError, PyToPrimitive, PyValue, Rc, Result,
        Value, ValueKind, effective_builtin_receiver, invoke_class_method, lookup_class_attr,
        range_len,
    };
    include!("builtins/length.rs");
}

mod ordering {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError, PyValueRef,
        Result, SortKind, Value, classify_sort, min_max_impl,
    };
    include!("builtins/ordering.rs");
}

mod rounding {
    use super::{
        ExpandedCallArg, FN_PREFIX, Interpreter, MODULE_NAME, OptValueRef, OptionalArg, PyBigInt,
        PyError, PyValueRef, Rc, Result, Value, ValueKind, coerce_numeric, invoke_class_method,
        lookup_class_attr, py_round_half_even_checked, round_bigint_neg_ndigits,
        round_float_ndigits, value_type_name_str,
    };
    include!("builtins/rounding.rs");
}

mod sequence_constructors {
    use super::{ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, Result, Value};
    include!("builtins/sequence_constructors.rs");
}

mod bytes_constructors {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError, Result, Value,
        ValueKind, bind_bytes_like_slots, bytes_count_via_index, bytes_element_to_u8,
        bytes_from_items, effective_builtin_receiver, encode_str_to_bytes, invoke_class_method,
        lookup_value_special_method, try_fast_bytes_elems, value_type_name_str,
    };
    include!("builtins/bytes_constructors.rs");
}

mod complex_constructor {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError, PyToPrimitive,
        Result, Value, ValueKind, coerce_subclass_backing, invoke_class_method,
        lookup_value_special_method, normalize_complex_slot_result, normalize_float_slot_result,
        parse_complex_str, value_to_float, value_type_name_str,
    };
    include!("builtins/complex_constructor.rs");
}

mod collection_constructors {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PySet, Result, Value,
    };
    include!("builtins/collection_constructors.rs");
}

mod text_constructor {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError, Result, Value,
        ValueKind, render_instance_str,
    };
    include!("builtins/text_constructor.rs");
}

mod int_constructor {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError, Result, Value,
        ValueKind, coerce_subclass_backing, float_to_bigint, int_parse_base_zero,
        int_parse_bytes_like, int_strip_explicit_base, invoke_class_method,
        lookup_value_special_method, normalize_int_slot_result, value_type_name_str,
    };
    include!("builtins/int_constructor.rs");
}

mod float_constructor {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError, PyToPrimitive,
        Result, Value, ValueKind, coerce_subclass_backing, float_bytes_like,
        float_parse_bytes_like, invoke_class_method, lookup_value_special_method,
        normalize_float_slot_result, pep515_strip_float, value_to_float, value_type_name_str,
    };
    include!("builtins/float_constructor.rs");
}

mod services {
    use super::{
        BuiltinTypeClass, ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg,
        PyDict, PyError, PyKey, PyStr, PyValue, PyValueRef, Rc, Result, Value, ValueKind,
        builtin_type_new, class_is_subclass_of, effective_builtin_receiver, invoke_class_method,
        is_dict_subclass_instance, is_not_iterable_error, lookup_class_attr,
        mapping_pairs_via_protocol, render_instance_str, resolve_zero_arg_super,
        type_class_singleton, value_type_name_str,
    };
    include!("builtins/services.rs");
}

mod container_protocols {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyDict, PyError, PyKey, Rc, Result, Value,
        ValueKind, builtin_data_backing, effective_builtin_receiver, instance_builtin_data,
        invoke_class_method, lookup_class_attr,
    };
    include!("builtins/container_protocols.rs");
}

mod object_basics {
    use super::{
        ExpandedCallArg, FN_PREFIX, InstanceAttrs, MODULE_NAME, PyError, Rc, Result, Value,
        ValueKind, class_has_native_builtin_type_ancestor, dir_names,
        find_immutable_primitive_base, find_mutable_primitive_base, find_scalar_primitive_base,
        full_type_name_str, getitem_iterator_reduce, hash_value_with_interp, is_getitem_iterator,
        is_reversed_iterator, lookup_class_attr, native_iterator_class, native_iterator_reduce,
        reject_keyword_args_expanded, render_value_repr, reversed_iterator_reduce,
        value_type_name_str,
    };
    include!("builtins/object_basics.rs");
}

mod type_protocols {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyClass, PyDict, PyError, PyKey, Rc, RefCell,
        Result, Value, ValueKind, primitive_class_dispatch, type_class_singleton,
    };
    include!("builtins/type_protocols.rs");
}

mod primitive_construction_protocols {
    use super::{
        ExpandedCallArg, FN_PREFIX, InstanceAttrs, MODULE_NAME, PyError, PySet, Rc, Result, Value,
        ValueKind, check_new_subtype, value_type_name_str,
    };
    include!("builtins/primitive_construction_protocols.rs");
}

mod object_comparison {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyError, Result, Value, ValueKind,
        apply_format_spec, apply_format_spec_named, builtin_data_backing, full_type_name_str,
        render_instance_str, render_value_repr, value_type_name_str,
    };
    include!("builtins/object_comparison.rs");
}

mod operators {
    use super::{
        BinaryOp, ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyDict, PyError, Rc, Result, Value,
        ValueKind, coerce_numeric, coerce_subclass_backing, effective_builtin_receiver,
        float_to_bigint, value_type_name_str,
    };
    include!("builtins/operators.rs");
}

mod object_protocols {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyError, Rc, Result, Value, ValueKind,
        value_type_name_str,
    };
    include!("builtins/object_protocols.rs");
}

mod exception_protocols {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyError, Rc, Result, Value, ValueKind,
        base_exception_reduce_value, classify_exception_class, unicode_exc_set_attrs,
        value_type_name_str,
    };
    include!("builtins/exception_protocols.rs");
}

mod importing {
    use super::{
        ExpandedCallArg, FN_PREFIX, MODULE_NAME, OptValueRef, OptionalArg, PyError, PyValueRef,
        Result, Value, ValueKind,
    };
    include!("builtins/importing.rs");
}

mod typing_protocols {
    use super::{ExpandedCallArg, FN_PREFIX, MODULE_NAME, PyError, Result, Value, ValueKind};
    include!("builtins/typing_protocols.rs");
}

/// Combined registry for the independently maintained builtin families.
pub(crate) fn regs() -> &'static [crate::builtin_registry::BuiltinReg] {
    static REGS_CELL: std::sync::LazyLock<Vec<crate::builtin_registry::BuiltinReg>> =
        std::sync::LazyLock::new(|| {
            let mut regs = Vec::new();
            regs.extend_from_slice(scalar::regs());
            regs.extend_from_slice(identity_and_repr::regs());
            regs.extend_from_slice(numeric_functions::regs());
            regs.extend_from_slice(aggregation::regs());
            regs.extend_from_slice(iteration::regs());
            regs.extend_from_slice(reflection::regs());
            regs.extend_from_slice(length::regs());
            regs.extend_from_slice(ordering::regs());
            regs.extend_from_slice(rounding::regs());
            regs.extend_from_slice(sequence_constructors::regs());
            regs.extend_from_slice(bytes_constructors::regs());
            regs.extend_from_slice(complex_constructor::regs());
            regs.extend_from_slice(collection_constructors::regs());
            regs.extend_from_slice(text_constructor::regs());
            regs.extend_from_slice(int_constructor::regs());
            regs.extend_from_slice(float_constructor::regs());
            regs.extend_from_slice(services::regs());
            regs.extend_from_slice(container_protocols::regs());
            regs.extend_from_slice(object_basics::regs());
            regs.extend_from_slice(type_protocols::regs());
            regs.extend_from_slice(primitive_construction_protocols::regs());
            regs.extend_from_slice(object_comparison::regs());
            regs.extend_from_slice(operators::regs());
            regs.extend_from_slice(object_protocols::regs());
            regs.extend_from_slice(exception_protocols::regs());
            regs.extend_from_slice(importing::regs());
            regs.extend_from_slice(typing_protocols::regs());
            regs
        });
    REGS_CELL.as_slice()
}

include!("builtins/namespace.rs");
include!("builtins/constructor_support.rs");
include!("builtins/reflection_support.rs");
include!("builtins/ordering_support.rs");
include!("builtins/type_checks.rs");
include!("builtins/parsing_bytes.rs");
include!("builtins/execution_support.rs");

#[cfg(test)]
mod family_boundary_tests {
    const FAMILY_SOURCES: &str = concat!(
        include_str!("builtins/scalar.rs"),
        include_str!("builtins/identity_and_repr.rs"),
        include_str!("builtins/numeric_functions.rs"),
        include_str!("builtins/aggregation.rs"),
        include_str!("builtins/iteration.rs"),
        include_str!("builtins/reflection.rs"),
        include_str!("builtins/length.rs"),
        include_str!("builtins/ordering.rs"),
        include_str!("builtins/rounding.rs"),
        include_str!("builtins/sequence_constructors.rs"),
        include_str!("builtins/bytes_constructors.rs"),
        include_str!("builtins/complex_constructor.rs"),
        include_str!("builtins/collection_constructors.rs"),
        include_str!("builtins/text_constructor.rs"),
        include_str!("builtins/int_constructor.rs"),
        include_str!("builtins/float_constructor.rs"),
        include_str!("builtins/services.rs"),
        include_str!("builtins/container_protocols.rs"),
        include_str!("builtins/object_basics.rs"),
        include_str!("builtins/type_protocols.rs"),
        include_str!("builtins/primitive_construction_protocols.rs"),
        include_str!("builtins/object_comparison.rs"),
        include_str!("builtins/operators.rs"),
        include_str!("builtins/object_protocols.rs"),
        include_str!("builtins/exception_protocols.rs"),
        include_str!("builtins/importing.rs"),
        include_str!("builtins/typing_protocols.rs"),
    );

    /// Issue #3294: the namespace is composed by visiting each family's
    /// exports directly.  No family `module()` is built to be merged and
    /// thrown away.
    #[test]
    fn namespace_is_composed_without_family_modules() {
        const FACADE: &str = include_str!("builtins.rs");
        const COMPOSER: &str = include_str!("builtins/namespace.rs");
        let families = FACADE
            .lines()
            .filter_map(|line| line.strip_prefix("mod ")?.strip_suffix(" {"))
            .filter(|name| *name != "family_boundary_tests")
            .collect::<Vec<_>>();
        assert_eq!(families.len(), 27, "builtin family list changed");
        for family in families {
            let family_module = format!("{family}::module()");
            assert!(
                !FACADE.contains(&family_module) && !COMPOSER.contains(&family_module),
                "{family} is built as a throwaway module instead of being visited in place"
            );
            assert_eq!(
                COMPOSER
                    .matches(&format!("{family}::for_each_function_export("))
                    .count(),
                1,
                "{family} must contribute to the builtins namespace exactly once"
            );
        }
    }

    #[test]
    fn module_stores_only_flat_functions_and_constants() {
        let module = super::module();
        let super::ValueKind::PyModule(module) = module.kind() else {
            panic!("builtins must be a module");
        };
        let module = module.borrow();
        assert!(module.has_pending_lazy_attrs());
        assert!(
            module.attrs.len() <= super::EAGER_SLOT_CAPACITY,
            "raise EAGER_SLOT_CAPACITY so the startup map is sized in one allocation"
        );
        for (name, value) in &module.attrs {
            assert!(
                !name.contains('.'),
                "{name} is a registry key, not a builtins name"
            );
            assert!(
                !matches!(value.kind(), super::ValueKind::PyClass(_))
                    && super::builtin_type_slot(name).is_none(),
                "{name} must stay deferred until first use"
            );
        }
        // Every flat registration is still reachable, whether it is stored
        // as a function token or resolved as the type of the same name.
        for registration in super::regs() {
            if !registration.name.contains('.') {
                assert!(
                    module.get_attr_value(registration.name).is_some(),
                    "builtins.{} is missing",
                    registration.name
                );
            }
        }
    }

    #[test]
    fn builtin_families_declare_parent_dependencies_explicitly() {
        let wildcard_parent_import = concat!("use super::", "*");
        assert!(
            !FAMILY_SOURCES.contains(wildcard_parent_import),
            "builtin family contains a wildcard parent import; list the \
             services owned by the family facade"
        );
    }

    #[test]
    fn constructor_families_stay_responsibility_sized() {
        const MAX_FAMILY_LINES: usize = 500;
        const CONSTRUCTOR_FAMILIES: &[(&str, &str, &[&str])] = &[
            (
                "sequence_constructors.rs",
                include_str!("builtins/sequence_constructors.rs"),
                &["list", "tuple"],
            ),
            (
                "bytes_constructors.rs",
                include_str!("builtins/bytes_constructors.rs"),
                &["bytes", "bytearray"],
            ),
            (
                "complex_constructor.rs",
                include_str!("builtins/complex_constructor.rs"),
                &["complex"],
            ),
            (
                "collection_constructors.rs",
                include_str!("builtins/collection_constructors.rs"),
                &["set", "frozenset"],
            ),
            (
                "text_constructor.rs",
                include_str!("builtins/text_constructor.rs"),
                &["str"],
            ),
            (
                "int_constructor.rs",
                include_str!("builtins/int_constructor.rs"),
                &["int"],
            ),
            (
                "float_constructor.rs",
                include_str!("builtins/float_constructor.rs"),
                &["float"],
            ),
        ];

        for (name, source, expected_functions) in CONSTRUCTOR_FAMILIES {
            let line_count = source.lines().count();
            assert!(
                line_count <= MAX_FAMILY_LINES,
                "{name} has {line_count} lines; split constructor responsibilities further"
            );

            let declared_functions = source
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("    fn ")
                        .and_then(|rest| rest.split_once('('))
                        .map(|(function, _)| function)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                declared_functions, *expected_functions,
                "{name} owns constructors from another responsibility family"
            );
        }
    }

    #[test]
    fn constructor_registration_order_stays_stable() {
        const EXPECTED: &[&str] = &[
            "list",
            "tuple",
            "bytes",
            "bytearray",
            "complex",
            "set",
            "frozenset",
            "str",
            "int",
            "float",
        ];
        let actual = super::regs()
            .iter()
            .filter_map(|registration| {
                EXPECTED
                    .contains(&registration.name)
                    .then_some(registration.name)
            })
            .collect::<Vec<_>>();

        assert_eq!(actual, EXPECTED);
    }

    #[test]
    fn numeric_constructors_gate_backing_by_the_current_slot_owner() {
        let int_source = include_str!("builtins/int_constructor.rs")
            .chars()
            .filter(|char| !char.is_whitespace())
            .collect::<String>();
        let float_source = include_str!("builtins/float_constructor.rs")
            .chars()
            .filter(|char| !char.is_whitespace())
            .collect::<String>();
        let complex_source = include_str!("builtins/complex_constructor.rs")
            .chars()
            .filter(|char| !char.is_whitespace())
            .collect::<String>();

        assert!(int_source.contains("coerce_subclass_backing(&self_val,&[\"__int__\"])",));
        assert!(float_source.contains("coerce_subclass_backing(&self_val,&[\"__float__\"])",));
        assert!(complex_source.contains("coerce_subclass_backing(&self_val,&[\"__complex__\"])",));
        assert!(complex_source.contains("coerce_subclass_backing(self_val,&[\"__float__\"])",));

        for source in [&int_source, &float_source, &complex_source] {
            assert!(
                !source.contains("instance_builtin_data"),
                "numeric constructors must use the owner-aware backing boundary"
            );
            assert!(
                !source.contains("invoke_numeric_constructor_slot"),
                "numeric slots must be invoked with the original receiver"
            );
        }
    }
}
