// Python generator, coroutine, await, and yield-from protocols.
//
// The register VM owns frame storage and resumption. This module owns the
// Python-visible protocol names, forwarding rules, termination conversion, and
// generator/coroutine method behaviour layered on top of that VM service.

include!("generator_protocols/classification.rs");
include!("generator_protocols/errors.rs");
include!("generator_protocols/attributes.rs");
include!("generator_protocols/coroutines.rs");
include!("generator_protocols/methods.rs");
