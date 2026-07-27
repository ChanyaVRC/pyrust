// Constant-related optimizer passes, composed in pipeline order.

include!("constant_folding/jump_threading.rs");
include!("constant_folding/binop_const_fusion.rs");
include!("constant_folding/tuple_folding.rs");
include!("constant_folding/comparison_jump_fusion.rs");
include!("constant_folding/constant_propagation.rs");
include!("constant_folding/dead_code.rs");
include!("constant_folding/unary_folding.rs");
include!("constant_folding/string_method_folding.rs");
include!("constant_folding/branch_folding.rs");
