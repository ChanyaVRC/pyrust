// Interpreter-aware boundary for concrete Python built-in methods.
//
// The generic callable router and the register VM hand a validated receiver,
// arguments, and method name to this module.  Type-specific names, protocol
// slots, live-view construction, and adapters that may execute Python code are
// owned here.

pub(crate) use pyrust_builtins::classmethod::NativeClassMethodCachePlan;

/// Reject keyword arguments on a builtin method that accepts none, raising the
/// CPython-matching `"<label>() takes no keyword arguments"` `TypeError`.
///
/// The label is only built when `kw` is non-empty, so the common empty-keyword
/// path does not allocate. The macro returns early from a `Result` function.
macro_rules! reject_kwargs {
    ($kw:expr, $($label:tt)+) => {
        if !$kw.is_empty() {
            return Err(pyrust_core::type_err!(
                "{}() takes no keyword arguments",
                format_args!($($label)+)
            ));
        }
    };
}

include!("builtin_methods/join_errors.rs");
include!("builtin_methods/slot_tables.rs");
include!("builtin_methods/object_protocol.rs");
include!("builtin_methods/protocol.rs");
include!("builtin_methods/unbound.rs");
include!("builtin_methods/print.rs");
include!("builtin_methods/int_bytes.rs");
include!("builtin_methods/type_subscripts.rs");
include!("builtin_methods/type_constructors.rs");
include!("builtin_methods/range.rs");
include!("builtin_methods/sequences.rs");
include!("builtin_methods/iterables.rs");
include!("builtin_methods/containers.rs");
include!("builtin_methods/text_bytes.rs");
include!("builtin_methods/str_kwargs.rs");
include!("builtin_methods/bound.rs");
include!("builtin_methods/bound_objects.rs");
include!("builtin_methods/bound_instances.rs");
include!("builtin_methods/bound_classes.rs");
include!("builtin_methods/bound_ranges.rs");
include!("builtin_methods/container_dispatch.rs");
include!("builtin_methods/callable_presentation.rs");
include!("builtin_methods/attribute_adapters.rs");
include!("builtin_methods/typing.rs");
include!("builtin_methods/callable_adapters.rs");
