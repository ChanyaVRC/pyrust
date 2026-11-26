// Runtime class construction owns function object creation for class bodies,
// metaclass selection, namespace execution, MRO entry resolution, __set_name__,
// and __init_subclass__. Calling an already-created class remains in `calls`.

/// Resolved class bases: `(primary_base, extra_bases)` as produced by
/// `make_class_resolve_bases`.
type ResolvedBases = (Option<Rc<RefCell<PyClass>>>, Vec<Rc<RefCell<PyClass>>>);

/// Runtime service run for every new class just before `__init_subclass__`
/// dispatch: CPython's `Generic.__init_subclass__` step, which records
/// `__parameters__` on `typing.Generic` subclasses (issue #3326).
///
/// The runtime owns only the typed signature; `typing` installs its
/// implementation on import, which necessarily precedes the creation of any
/// `Generic` subclass.
pub(crate) type GenericSubclassHook = fn(&mut Interpreter, &Rc<RefCell<PyClass>>) -> Result<()>;

/// Process-wide because the hook is a plain function pointer with no state.
static GENERIC_SUBCLASS_HOOK: std::sync::OnceLock<GenericSubclassHook> = std::sync::OnceLock::new();

/// Install the `Generic` subclass hook (idempotent: the first install wins).
pub(crate) fn install_generic_subclass_hook(hook: GenericSubclassHook) {
    let _ = GENERIC_SUBCLASS_HOOK.set(hook);
}

/// Products of executing one class body. Keeping the annotation-scope handles
/// beside the namespace and `__class__` environment prevents either caller
/// from forgetting the PEP 695 finalization step.
struct ClassBodyExecution {
    attrs: IndexMap<String, Value>,
    class_env: EnvRef,
    annotation_scopes: Vec<Weak<RefCell<Environment>>>,
}

include!("classes/primitive_layout.rs");
include!("classes/annotation_scopes.rs");
include!("classes/entry.rs");
include!("classes/metaclass.rs");
include!("classes/bases.rs");
include!("classes/finalization.rs");
