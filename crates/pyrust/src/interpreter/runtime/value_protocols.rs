// Cross-cutting Python value protocols.
//
// These services are independent of a particular syntax form, builtin method,
// or VM instruction. Callers supply context-specific diagnostics while this
// boundary owns protocol lookup and result validation.

include!("value_protocols/backing.rs");
include!("value_protocols/operators.rs");
include!("value_protocols/index.rs");
include!("value_protocols/length.rs");
