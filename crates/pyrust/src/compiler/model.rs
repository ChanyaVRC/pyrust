// Compiler state and its independent syntax-analysis helpers share this module's
// private lexical scope.
include!("model/state.rs");
include!("model/class_annotations.rs");
include!("model/yield_scan.rs");
include!("model/async_scan.rs");
include!("model/expression_text.rs");
include!("model/pattern_bindings.rs");
include!("model/call_shapes.rs");
