// Register-VM composition.
//
// Owns register/frame state, bytecode execution and unwinding, and low-level
// frame resumption. It may consume typed specializations from the sibling
// `fast_path` domain, but does not own their policy. Generator/coroutine
// protocols, type objects, and collection mutation guards are separate
// modules.

include!("vm/state.rs");
include!("vm/entry.rs");
include!("vm/execute.rs");
include!("vm/helpers.rs");
