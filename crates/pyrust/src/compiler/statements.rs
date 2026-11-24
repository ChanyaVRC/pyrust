// Statement compiler implementation grouped by semantic responsibility.

include!("statements/dispatch.rs");
include!("statements/type_aliases.rs");
include!("statements/assignments.rs");
include!("statements/unpack_and_aug.rs");
include!("statements/dead_code_analysis.rs");
