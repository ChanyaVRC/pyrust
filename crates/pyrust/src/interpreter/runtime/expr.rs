// Expression evaluation is grouped by protocol and operation family. These
// includes intentionally share the runtime module's private helper namespace.

include!("expr/unary.rs");
include!("expr/numeric_slots.rs");
include!("expr/indexing.rs");
include!("expr/binary_dispatch.rs");
include!("expr/binary_arithmetic.rs");
include!("expr/binary_inplace.rs");
include!("expr/binary_numeric_tail.rs");
include!("expr/slicing_fast.rs");
include!("expr/slicing_membership.rs");
include!("expr/complex_union.rs");
include!("expr/item_ops.rs");
