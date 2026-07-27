// Call-runtime composition.
//
// This boundary owns generic callable resolution, Python argument binding, and
// construction through a callable class. Concrete built-in methods, formatting,
// exception presentation, and class-statement execution are separate modules.

include!("calls/parameter_binding.rs");
include!("calls/keyword_arguments.rs");
include!("calls/function_cache.rs");
include!("calls/generator_construction.rs");
include!("calls/binding_analysis.rs");
include!("calls/user_frame.rs");
include!("calls/expanded_user_call.rs");
include!("calls/variadic_user_call.rs");
include!("calls/instances.rs");
include!("calls/construction_plan.rs");
