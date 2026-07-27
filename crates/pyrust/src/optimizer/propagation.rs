// Data-flow propagation passes.

include!("propagation/copy.rs");
include!("propagation/loop_inversion.rs");
include!("propagation/load_none_merging.rs");
include!("propagation/constant_pool.rs");
