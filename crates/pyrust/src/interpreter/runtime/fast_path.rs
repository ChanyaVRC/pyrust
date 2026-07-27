// Fast paths are grouped by the runtime operation they specialize. The include
// fragments share this domain's private scope, while `runtime.rs` keeps the
// domain itself separate from the opcode execution module.

include!("fast_path/unary.rs");
include!("fast_path/call_cache_support.rs");
include!("fast_path/global_load.rs");
include!("fast_path/loop_iteration.rs");
include!("fast_path/binary_support.rs");
include!("fast_path/control_flow.rs");
include!("fast_path/collection_support.rs");
include!("fast_path/binop_and_getattr.rs");
include!("fast_path/keyword_calls.rs");
include!("fast_path/expanded_calls.rs");
include!("fast_path/call_binding.rs");
include!("fast_path/setattr.rs");
include!("fast_path/cache_support.rs");
include!("fast_path/method_cache.rs");
include!("fast_path/method_call.rs");
