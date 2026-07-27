// Runtime exception construction, coercion, chaining, group semantics, and
// presentation. The VM owns handler-stack control transfer; exception object
// representation and Python-visible state stay here.

include!("exceptions/construction.rs");
include!("exceptions/control.rs");
include!("exceptions/groups.rs");
include!("exceptions/render.rs");
include!("exceptions/unicode_args.rs");
include!("exceptions/slots.rs");
