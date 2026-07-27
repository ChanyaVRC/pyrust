// Runtime class construction owns function object creation for class bodies,
// metaclass selection, namespace execution, MRO entry resolution, __set_name__,
// and __init_subclass__. Calling an already-created class remains in `calls`.

/// Resolved class bases: `(primary_base, extra_bases)` as produced by
/// `make_class_resolve_bases`.
type ResolvedBases = (Option<Rc<RefCell<PyClass>>>, Vec<Rc<RefCell<PyClass>>>);

include!("classes/primitive_layout.rs");
include!("classes/entry.rs");
include!("classes/metaclass.rs");
include!("classes/bases.rs");
include!("classes/finalization.rs");
