// Python dict/set key semantics.
//
// Owns hashing, equality-aware bucket lookup, and key-preserving mutation.
// Collection algebra and expression evaluation consume this API but do not
// duplicate the object-key slow paths.

include!("collection_keys/hashing.rs");
include!("collection_keys/object_keys.rs");
include!("collection_keys/equality_fast.rs");
include!("collection_keys/key_conversion.rs");
include!("collection_keys/lookup.rs");
include!("collection_keys/mutation.rs");
include!("collection_keys/value_equality.rs");
