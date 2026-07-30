// Runtime domain boundaries are documented in `runtime/ARCHITECTURE.md`.

mod program_execution {
    use super::{
        CallDepthGuard, EnvRef, Environment, FrameKind, HashMap, IntMaxStrDigitsExecutionGuard,
        Interpreter, PyDict, PyError, PyKey, Rc, RegSlice, RegsBuf, Result, Stmt, StrKey, Value,
        ValueKind, VmFrameView, cached_builtins_module, exception_str_with_dispatch,
        format_exc_chain_prefix, intern_string, module_env, render_instance_repr, smallvec,
    };
    include!("runtime/program_execution.rs");
}

mod value_protocols {
    use super::{
        ExpandedCallArg, Interpreter, PyError, PyInstance, Rc, RefCell, Result, Value, ValueKind,
        builtin_data_backing, class_is_subclass_of, compare_values, compare_values_with_op,
        effective_builtin_receiver, invoke_class_method, lookup_class_attr, metaclass_dunder,
        value_type_name_str,
    };
    include!("runtime/value_protocols.rs");
}
pub(crate) use value_protocols::{
    coerce_numeric, coerce_subclass_backing, lookup_value_special_method,
    normalize_complex_slot_result, normalize_float_slot_result, normalize_int_slot_result,
    slot_is_callable,
};
use value_protocols::{coerce_operand_backing, is_callable_method, is_not_implemented};

mod formatting {
    use super::{
        ExpandedCallArg, HashMap, Interpreter, PyBigInt, PyError, PyInstance, PyKey, Rc, RefCell,
        Result, Value, ValueKind, ascii_repr_interp, bigint_to_float_or_overflow,
        builtin_data_backing, exception_str_with_dispatch, extract_str_value, float_to_bigint,
        full_type_name_str, instance_builtin_data, intern_string, invoke_class_method,
        is_exception_class, is_percent_format_mapping, is_str_or_str_subclass, lookup_class_attr,
        lookup_value_special_method, normalize_float_slot_result, render_instance_repr,
        try_value_to_float, value_type_name_str,
    };
    include!("runtime/formatting.rs");
}
pub(crate) use formatting::{
    CallBuiltinCacheEntry, FmtSpecCacheEntry, apply_format_spec, apply_format_spec_named,
    format_bin_i64, format_oct_i64, render_instance_str, render_key_repr, render_value_repr,
};
use formatting::{format_dunder_owner, format_dunder_spec_arg};

mod exceptions {
    use super::{
        ExpandedCallArg, Interpreter, PyClass, PyError, PyInstance, PyToPrimitive, Rc, RefCell,
        Result, Value, ValueKind, builtin_data_backing, canonical_class_by_tag,
        class_is_builtin_exception_subclass, class_is_subclass_of, classify_exception_class,
        instantiate_attribute_error, instantiate_exception, instantiate_exception_with_kinds,
        instantiate_import_error, instantiate_name_error, instantiate_os_error,
        instantiate_unicode_decode_error, instantiate_unicode_encode_error, invoke_class_method,
        is_exception_class, is_stop_iteration_error, lookup_class_attr, lookup_exc_class,
        lookup_value_special_method, reject_keyword_args_expanded, render_key_repr,
        render_value_repr, value_to_bigint, value_type_name_str,
    };
    include!("runtime/exceptions.rs");
}
use exceptions::{ExceptionSlotPolicy, exception_slot_policy, render_instance_repr};
pub(crate) use exceptions::{
    ascii_repr_interp, exception_repr_with_dispatch, exception_str_with_dispatch,
    is_sequence_iter_terminator,
};

mod call_state {
    use super::{Cell, Interpreter};
    include!("runtime/call_state.rs");
}
use call_state::{CallDepthGuard, IntMaxStrDigitsExecutionGuard, call_depth, max_call_depth};
pub(crate) use call_state::{
    get_call_depth, get_int_max_str_digits, get_recursion_limit, set_int_max_str_digits,
    set_recursion_limit,
};

mod calls {
    use super::classes::{PrimitiveLayout, classify_primitive_layout, primitive_layout_for_class};
    use super::{
        BUILTIN_DATA_ATTR, CallDepthGuard, EnvRef, ExcHandlersBuf, ExpandedArgBuf, ExpandedCallArg,
        FnCode, FrameKind, GeneratorFrame, GeneratorKind, HandledExcBuf, HashMap, InstanceAttrs,
        Interpreter, PyClass, PyDict, PyError, PyInstance, PyKey, Rc, RefCell, RegSlice, RegsBuf,
        Result, UserFunction, Value, ValueKind, VmFrameView, call_depth,
        class_chain_new_slot_wrapped, class_is_subclass_of, invoke_class_method,
        is_exception_class, is_primitive_class, lookup_class_attr, mapping_entries_for_expansion,
        max_call_depth, object_class_singleton, smallvec, type_class_singleton,
        value_type_name_str,
    };
    include!("runtime/calls.rs");
}
use calls::{callable_error_name, duplicate_keyword_error};

mod call_dispatch {
    use super::{
        ExpandedCallArg, Interpreter, Rc, Result, Value, ValueKind, invoke_class_method,
        lookup_class_attr,
    };
    include!("runtime/call_dispatch.rs");
}

mod classes {
    use super::{
        ActiveClassAnnotationScopes, EnvRef, Environment, ExpandedArgBuf, ExpandedCallArg,
        FrameKind, HashMap, HashSet, IndexMap, Interpreter, PrimitiveClassKind, PyClass, PyDict,
        PyError, PyKey, Rc, RefCell, RegSlice, RegsBuf, Result, UserFunction, UserFunctionParam,
        Value, ValueKind, VmFrameView, Weak, class_is_subclass_of, class_mro_items,
        has_local_binding_in_current_or_ancestor, invoke_class_method, lookup_class_attr,
        make_slot_member_descriptor, metaclass_of, object_class_singleton, primitive_class_kind,
        smallvec, type_class_singleton, value_type_name_str, vm_read,
    };
    include!("runtime/classes.rs");
}

mod builtin_methods {
    use super::collection_ops::{
        dict_entries_from_value, set_has_object_key, set_items_from_value, set_subset_cmp,
        set_subtract_in_place, value_iterable_needs_runtime_key_semantics,
    };
    use super::{
        BUILTIN_DATA_ATTR, BinaryOp, ExpandedArgBuf, ExpandedCallArg, GeneratorKind, IndexMap,
        Interpreter, PrimitiveClassKind, PrintOptions, PyBigInt, PyClass, PyDict, PyError,
        PyInstance, PyKey, PySet, PyToPrimitive, Rc, RefCell, RegSlice, Result, Value, ValueKind,
        apply_format_spec, builtin_data_backing, builtin_iterator_has_length_hint,
        c3_linearize_classes, call_bytes_method_coerced_prevalidated, canonical_class_by_tag,
        class_descriptor_display_name, class_direct_subclasses, class_is_subclass_of,
        class_mro_items, coerce_bytes_subclass_arg, coerce_bytes_subclass_join_iterable,
        coerce_bytes_subclass_method_args, coerce_bytes_subclass_method_kwargs,
        coerce_str_subclass_arg, coerce_str_subclass_join_iterable,
        coerce_str_subclass_method_args, dispatch_property_method, eval_builtin_unary,
        extract_optional_string, format_dunder_owner, format_dunder_spec_arg,
        hash_value_with_interp, instance_builtin_data, invoke_class_method,
        is_ordered_dict_class_or_subclass, is_primitive_class, is_stop_iteration_error,
        lookup_class_attr, make_iterator, ordered_dict_owner, primitive_class_by_name,
        primitive_class_dispatch, primitive_class_kind, reject_keyword_args_expanded,
        render_instance_repr, type_class_singleton, value_class, value_from_bigint,
        value_to_bigint, value_type_name_str,
    };
    include!("runtime/builtin_methods.rs");
}
pub(crate) use builtin_methods::{
    NativeClassMethodCachePlan, adapt_builtin_subclass_method, bind_builtin_attribute,
    bind_builtin_class_special, bind_cached_native_class_method, builtin_callable_metadata,
    builtin_callable_presentation, dir_names, is_builtin_callable_adapter,
    is_builtin_class_getitem_sentinel, is_object_protocol_method, make_builtin_generic_alias,
    primitive_owned_object_dunder,
};

mod collection_keys {
    use super::collection_ops::{is_setlike_view, set_has_object_key};
    use super::{
        Interpreter, PyDict, PyError, PyKey, PySet, Rc, Result, StrKey, Value, ValueKind,
        class_hash_inherits_builtin_none, coerce_numeric, coerce_subclass_backing,
        invoke_class_method, lookup_class_attr, py_hash_bigint, py_hash_float, py_hash_int,
        range_len, slot_is_callable, value_type_name_str,
    };
    include!("runtime/collection_keys.rs");
}
pub(crate) use collection_keys::hash_value_with_interp;
use collection_keys::{key_contains_object, nested_object_tuple_key, value_needs_slow_hash};

mod collection_ops {
    use super::{
        BinaryOp, Interpreter, PyKey, PySet, Result, Value, ValueKind, builtin_data_backing,
        key_contains_object, mapping_pairs_via_protocol, value_needs_slow_hash,
        value_type_name_str,
    };
    include!("runtime/collection_ops.rs");
}
use collection_ops::mapping_entries_for_expansion;

mod fast_path {
    use super::builtin_methods::BuiltinContainerKind;
    use super::execution::vm_read;
    use super::{
        BigRangeState, BinaryOp, CallBuiltinCacheEntry, ExpandedCallArg, GlobalCacheEntry,
        GlobalResolutionCache, Interpreter, IterState, LiveDictViewItem, MemoKey,
        NativeClassMethodCachePlan, PyDict, PyError, PyKey, Rc, ReadAttributeCachePlan,
        ReadMethodCachePlan, RegSlice, Result, UserFunction, Value, ValueKind,
        bind_builtin_attribute, bind_cached_native_class_method, comp_read_is_free, float_divmod,
        indexed_sequence_item, invoke_class_method, live_collection_len, live_dict_view_item,
        ordered_mapping_guard_outcome, py_mod_i64, read_attribute_cache_plan,
        read_method_cache_plan, value_from_bigint, write_attribute_cache_class,
    };
    include!("runtime/fast_path.rs");
}

mod expressions {
    use super::builtin_methods::i64_range_contains;
    use super::collection_ops::{
        SetOp, bitor_operand_type_name, dict_entries_from_value, is_mapping_proxy, set_binary_op,
        set_has_object_key, set_items_from_value, set_subset_cmp, set_subtract_in_place,
    };
    use super::fast_path::try_tagged_int_unary;
    use super::{
        BinaryOp, ExpandedCallArg, Interpreter, PyBigInt, PyBigIntSign, PyDict, PyError, PyKey,
        PyPow, PySet, PyToPrimitive, PyZero, Rc, RegSlice, Result, UnaryOp, Value, ValueKind,
        builtin_data_backing, class_is_subclass_of, coerce_bytes_subclass_arg, coerce_numeric,
        coerce_operand_backing, coerce_str_subclass_arg, compare_values_with_op,
        effective_builtin_receiver, float_divmod, int_pow_promoting, invoke_class_method,
        is_async_generator_value, is_builtin_class_getitem_sentinel, is_callable_method,
        is_coroutine_value, is_not_implemented, is_stop_iteration_error, is_type_alias_class,
        key_contains_object, lookup_class_attr, make_builtin_generic_alias, make_slice_value,
        metaclass_dunder, nested_object_tuple_key, normalize_index, normalize_index_write,
        py_mod_i64, range_len, type_class_singleton, value_type_name_str, values_are_identical,
        vm_read,
    };
    include!("runtime/expr.rs");
}
use expressions::bigint_to_float_or_overflow;
pub(crate) use expressions::{
    bigint_divmod_floor, dispatch_numeric_binop, eval_builtin_unary, value_from_bigint,
    value_to_bigint,
};

mod attributes {
    use super::{
        ExceptionSlotPolicy, ExpandedCallArg, Interpreter, PyClass, PyDict, PyError, PyInstance,
        PyKey, Rc, RefCell, Result, StrKey, UserFunction, UserFunctionKind, Value, ValueKind,
        adapt_builtin_subclass_method, bind_builtin_class_special, builtin_callable_metadata,
        builtin_class_doc, canonical_class_by_tag, class_chain_new_slot_wrapped,
        class_hash_inherits_builtin_none, class_is_subclass_of, class_suppresses_instance_dict,
        descriptor_get_slot_raw_call, exception_slot_policy, instance_builtin_data,
        invoke_class_method, is_exception_class, is_type_alias_class, is_typevar_class,
        lookup_class_attr, metaclass_dunder, metaclass_of, mro_has_unslotted_ancestor,
        mro_slot_allows, object_class_singleton, primitive_owned_object_dunder,
        replace_instance_dict, type_alias_readonly_attr_error, typevar_readonly_attr_error,
        value_class, value_type_name_str,
    };
    include!("runtime/attributes.rs");
}
pub(crate) use attributes::{
    ReadAttributeCachePlan, ReadMethodCachePlan, class_descriptor_display_name,
    dispatch_property_method, read_attribute_cache_plan, read_method_cache_plan,
    write_attribute_cache_class,
};
use attributes::{class_direct_subclasses, class_mro_items};

mod namespaces {
    use super::{
        CachedImportModuleRegistry, CachedModuleClass, CollectionMutationState, ENV_POOL_MAX,
        EnvRef, Environment, ExpandedCallArg, FrameKind, HashMap, InstanceAttrs, Interpreter,
        Lexer, MODULE_CLASS_CACHE_SLOT_COUNT, ModuleClassCache, ModuleClassCacheSlot,
        ModuleMutationState, Parser, PathBuf, PyClass, PyDict, PyError, PyInstance, PyKey,
        PyModule, Rc, RefCell, RegSlice, Result, StrKey, Value, ValueKind, cached_builtins_module,
        call_del_if_last_binding, env_assign_local, find_enclosing_local_env_for_name,
        get_int_max_str_digits, invoke_class_method, is_cached_builtins_module,
        lookup_name_in_enclosing_local_env, lookup_name_in_env, lookup_name_in_env_as_free,
        lookup_name_in_module, lookup_value_special_method, module_env, object_class_singleton,
        set_int_max_str_digits, value_type_name_str, values_are_identical, vm_read,
    };
    include!("runtime/namespaces.rs");
}
pub(crate) use namespaces::{GlobalResolutionCache, bump_global_struct_version, comp_read_is_free};

mod standard_streams {
    use super::{Interpreter, PyError, Result, Value, ValueKind};
    include!("runtime/standard_streams.rs");
}

mod truthiness {
    use super::{
        Interpreter, Rc, Result, Value, ValueKind, builtin_data_backing, coerce_numeric,
        invoke_class_method, lookup_class_attr, metaclass_dunder,
    };
    include!("runtime/truthiness.rs");
}

mod slicing {
    use super::{
        Interpreter, PyBigInt, PyBigIntSign, PyToPrimitive, Result, Value, ValueKind,
        value_to_bigint,
    };
    include!("runtime/slicing.rs");
}
pub(crate) use slicing::make_slice_value;

mod pattern_matching {
    use super::{Interpreter, Result, Value, ValueKind, class_is_subclass_of, value_type_name_str};
    include!("runtime/pattern_matching.rs");
}

mod iteration {
    use super::{
        ExpandedArgBuf, ExpandedCallArg, GenDriving, GeneratorCell, GeneratorFrame, Interpreter,
        PyBigInt, PyBigIntSign, PyError, PyInstance, PyKey, Rc, RefCell, Result, Value, ValueKind,
        builtin_data_backing, effective_user_iter, full_type_name_str,
        i64_range_native_cursor_safe, instance_builtin_data, instantiate_exception,
        invoke_class_method, is_coroutine_value, is_inherited_builtin_iter_sentinel,
        is_sequence_iter_terminator, is_stop_iteration_error, key_ref_to_value, key_to_value,
        lookup_class_attr, lookup_value_special_method, metaclass_dunder, range_len,
        value_from_bigint, value_to_bigint, value_type_name_str,
    };
    include!("runtime/iteration.rs");
}
pub(crate) use iteration::{
    BigRangeIter, BigRangeState, CallableIter, ConsumerIterator, EnumerateIter, FilterIter,
    GetItemIter, GuardVersion, IterCacheBuf, IterSrcBuf, IterState, IteratorCopy, ItersBuf,
    LiveDictViewItem, LoopIteratorAdvance, MapIter, NativeIterFrame, NativeIterGuard,
    ProviderIterator, RangeIter, ZipIter, builtin_iterator_has_length_hint, copy_iterator_object,
    indexed_sequence_item, iter_values, iterator_retained_values, live_collection_len,
    live_dict_view_item, make_iterator, make_reversed_dict_iter, make_reversed_getitem_iterator,
    make_reversed_mapping_snapshot_iter, make_reversed_range_iterator,
    make_reversed_sequence_iterator, ordered_mapping_guard_outcome, set_iterator_retained_values,
    value_has_length_hint,
};

mod type_objects {
    use super::{
        AsyncGenASend, BigRangeIter, CallableIter, EnumerateIter, FilterIter, GetItemIter,
        IndexMap, InstanceAttrs, MapIter, NativeIterFrame, ProviderIterator, PyClass, PyError,
        PyInstance, RangeIter, Rc, RefCell, Result, UserFunctionKind, Value, ValueKind, ZipIter,
        function_type_singleton, generic_alias_class_singleton, method_type_singleton,
        primitive_class_for_value, type_class_singleton, value_type_name_str,
    };
    include!("runtime/type_objects.rs");
}
pub(crate) use pyrust_builtins::ordered_mapping::owner as ordered_dict_owner;
pub(crate) use type_objects::{
    full_type_name_str, initialize_typevar_attr, is_type_alias_class, is_typevar_class,
    make_slot_member_descriptor, make_type_alias_from_syntax, make_typevar_from_syntax,
    type_alias_class_singleton, type_alias_readonly_attr_error, typevar_class_singleton,
    typevar_readonly_attr_error, value_class,
};

#[inline]
pub(crate) fn is_ordered_dict_class_or_subclass(class: &Rc<RefCell<PyClass>>) -> bool {
    pyrust_builtins::ordered_mapping::class_policy(class).is_some()
}

mod execution {
    use super::fast_path::{
        LoopFastOutcome, MemoCallProbe, advance_loop_fast_state, build_string_fast,
        iter_slot_is_indexed_sequence, iter_slot_is_int_range, iter_slot_is_int_range_exact,
        list_reserve_hint, try_builtin_sequence_len, try_constant_compare_fast,
        try_indexed_sequence_int_element, try_inline_leaf_binop, try_integer_compare_fast,
        try_scalar_truthiness_fast, try_tagged_int_unary, value_is_builtin_len,
    };
    use super::{
        CallDepthGuard, EnvRef, FrameKind, GeneratorCell, HashMap, Interpreter, IterCacheBuf,
        IterState, ItersBuf, LoopIteratorAdvance, MemoKey, PyError, Rc, RegSlice, Result, Value,
        ValueKind, VmFrameView, bump_global_struct_version, call_depth, callable_error_name,
        duplicate_keyword_error, extract_stop_iteration_value, initialize_typevar_attr,
        intern_string_value, is_stop_iteration_error, lookup_class_attr, make_slice_value,
        make_type_alias_from_syntax, make_typevar_from_syntax, max_call_depth,
        pep479_wrap_stop_iteration, value_type_name_str,
    };
    include!("runtime/vm.rs");
    include!("runtime/vm/tests.rs");
}
pub(crate) use execution::{
    AsyncGenASend, CoroStep, ExcHandlersBuf, GeneratorFrame, HandledExcBuf, RegsBuf,
};
use execution::{GenDriving, vm_read};

mod generator_protocols {
    use super::{
        AsyncGenASend, CoroStep, ExpandedCallArg, GenDriving, GeneratorCell, GeneratorFrame,
        GeneratorKind, GetItemIter, Interpreter, NativeIterFrame, PyError, Rc, Result, Value,
        ValueKind, class_is_builtin_exception_subclass, class_is_subclass_of, full_type_name_str,
        instantiate_exception, invoke_class_method, lookup_class_attr, lookup_exc_class,
        value_has_length_hint, value_type_name_str,
    };
    include!("runtime/generator_protocols.rs");
}
use generator_protocols::{extract_stop_iteration_value, pep479_wrap_stop_iteration};
pub(crate) use generator_protocols::{
    is_async_generator_value, is_coroutine_value, is_stop_iteration_error,
};

mod introspection {
    use super::{
        GeneratorFrame, Interpreter, Rc, UserFunction, Value, ValueKind,
        lookup_enclosing_function_value, snapshot_current_locals,
    };
    include!("runtime/introspection.rs");
}

#[cfg(test)]
mod ownership_tests {
    include!("runtime/ownership_tests.rs");
}
