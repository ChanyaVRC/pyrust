// Built-in collection algebra.
//
// Owns dict merge operands and set/frozenset algebra. Hashing, equality and
// key storage live in `collection_keys`; expression evaluation only selects
// the operator and delegates here.

include!("collection_ops/mapping.rs");
include!("collection_ops/operands.rs");
include!("collection_ops/set_fast.rs");
include!("collection_ops/set.rs");
