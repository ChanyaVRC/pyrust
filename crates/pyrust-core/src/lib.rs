//! Shared runtime object model for PyRust.
//!
//! The crate root is intentionally only a public facade.  Implementation
//! details live in responsibility-oriented Rust modules so adding a private
//! helper to one domain does not silently make it available to every other
//! domain.

#[path = "core/arguments.rs"]
mod arguments;
#[path = "core/class_epoch.rs"]
mod class_epoch;
#[path = "core/cycle_guards.rs"]
mod cycle_guards;
#[path = "core/environment.rs"]
mod environment;
#[path = "core/error_macros.rs"]
mod error_macros;
#[path = "core/errors.rs"]
mod errors;
#[path = "core/numeric_ops.rs"]
mod numeric_ops;
#[path = "core/object_identity.rs"]
mod object_identity;
#[path = "core/object_model.rs"]
mod object_model;
#[path = "core/pool.rs"]
mod pool;
#[path = "core/string_interning.rs"]
mod string_interning;
#[path = "core/symbol.rs"]
mod symbol;
#[path = "core/traceback.rs"]
mod traceback;

pub use arguments::{
    expect_arg_count, extract_fill_char, extract_int, extract_optional_int, extract_optional_str,
    extract_str,
};
pub use class_epoch::{
    bump_class_epoch, class_cache_stamp, class_cache_stamp_matches, class_epoch,
};
pub use cycle_guards::{CycleGuard, enter_repr_cycle_guard};
pub use environment::{
    CellSlot, EnvRef, EnvValues, Environment, EnvironmentStorage, NameSet,
    NamespaceFastLocalCacheSource, NamespaceMirrorGuard, namespace_name_interest_mask,
};
pub use errors::{PyError, Result};
pub use numeric_ops::{
    NumericBinOp, NumericCmpOp, NumericOps, NumericSlot, NumericUnaryOp, bigint_divmod_floor,
    bigint_float_cmp, bigint_float_eq, bigint_to_float_or_overflow, dispatch_numeric_binop,
    dispatch_numeric_bool, dispatch_numeric_compare, dispatch_numeric_unary, float_divmod,
    int_float_cmp, int_float_eq, int_pow_promoting, numeric_eq, numeric_partial_cmp, py_mod_i64,
    value_from_bigint, value_to_bigint,
};
pub use object_model::{
    BigRangeData, BuiltinCallablePresentation, BuiltinCallablePresentationProvider,
    BuiltinRegistry, BuiltinState, BuiltinTypeClassTag, BuiltinTypeOps, CachedConstructionPlan,
    CanonicalClassTag, CollectionMutationState, CompareValuesFn, DefaultsOverride,
    FilesystemModuleNamespace, FnNameOverrides, FrozenSetKey, GeneratorCell, GeneratorKind,
    GeneratorNames, INT_MAX_STR_DIGITS_DEFAULT, INT_MAX_STR_DIGITS_MIN, InstanceAttrs,
    IntMaxStrDigitsGuard, IterValuesFn, ListInner, MemberSlotId, ModuleAttrs, ModuleMutationState,
    Opaque, ParamBind, PyBigInt, PyBigIntSign, PyClass, PyDict, PyHasher, PyInstance, PyKey,
    PyModule, PyPow, PySet, PySetProbeSnapshot, PyToPrimitive, PyZero, SetInner, SortKind, StrKey,
    TupleInner, UserFunction, UserFunctionKind, UserFunctionParam, Value, ValueKeyRef, ValueKind,
    WeakValueCache, bigint_str_digits_exceed_limit, bigrange_eq, bigrange_len,
    builtin_callable_presentation, builtin_layout_type_name, builtin_ops_is, builtin_type_name,
    cesu8_codepoints, cesu8_encode_codepoint, cesu8_next_codepoint, check_int_parse_digits,
    check_int_str_conversion, class_chain_contains_builtin_exception,
    class_chain_contains_exception, classify_sort, compare_values_via_registry, cp_is_printable,
    dict_iteration_mutation_state, dict_iteration_mutation_state_with_len, error_type_name,
    format_unicode_decode_str, format_unicode_encode_str, format_unicode_translate_str,
    get_int_max_str_digits, i64_range_native_cursor_safe,
    install_builtin_callable_presentation_provider, install_builtin_registry,
    install_compare_values, install_iter_values, int_max_str_digits_format_error,
    int_str_base_is_exempt, intern_attr_key, is_exception_instance, iter_values_via_registry,
    key_repr, lookup_builtin_ops, next_fn_id, py_hash_ellipsis, py_hash_nan, py_hash_none,
    py_hash_not_implemented, py_hash_pykey, py_value_display_name, range_len,
    scoped_int_max_str_digits, set_int_max_str_digits, value_may_exceed_int_str_limit,
};
pub use string_interning::{intern_string, intern_string_value};
pub use symbol::Symbol;
pub use traceback::{
    FrameGlobals, FrameInfo, captured_error_frames_len, clone_captured_error_frames,
    format_traceback, get_current_vm_col_span, get_current_vm_line, record_traceback_frame,
    reset_captured_error_frames, reset_current_vm_line, set_current_vm_col_span,
    set_current_vm_line, take_captured_error_frames,
};

#[cfg(test)]
#[path = "core/ownership_tests.rs"]
mod ownership_tests;
