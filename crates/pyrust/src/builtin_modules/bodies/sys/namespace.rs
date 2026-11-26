// Single-pass construction of the `sys` namespace (issue #3293).
//
// `STARTUP_SLOTS` describes every name `sys` binds at startup, in insertion
// order (issue #2918), followed by the callables `pyrust_module!` registers.
// `module()` stores only the cheap slots.  The heavy ones — `sys.monitoring`
// (a module, an `events` namespace and eleven callables), the 300-entry
// `sys.stdlib_module_names`, `sys.flags` and the struct-sequence style values
// — are left to `PyModule`'s deferred-attribute backing and are built the
// first time a script names them (or observes the whole namespace through
// `vars(sys)`, `dir(sys)` or `sys.__dict__`).  A script that never does
// allocates none of them.
//
// The deferred values belong to one `sys` module object (`per_module`), so two
// interpreters on a thread never share a mutable `sys.monitoring`; only the
// immutable `stdlib_module_names` frozenset and the classes of the
// struct-sequence style values are shared per GIL group.

use std::cell::RefCell;
use std::rc::Rc;

use super::{MODULE_NAME, PYRUST_COPYRIGHT, exports, monitoring, platform_info, version_info};
use crate::value::{LazyModuleAttrSource, LazyModuleSlotVisitor, ModuleAttrs, PyModule, Value};

/// How one startup slot gets its value.
enum Slot {
    /// Stored in the namespace when the module object is built.
    Eager(fn() -> Value),
    /// Built on first use; see [`resolve_deferred`].
    Deferred,
}

/// A slot `Interpreter::initialize_system_module` overwrites with the
/// importing interpreter's own value before the module is published.  It only
/// reserves the slot's position, so no throwaway container is allocated.
const PER_INTERPRETER: Slot = Slot::Eager(Value::none);

/// Every non-callable name bound in `sys` at startup, in insertion order.
///
/// The five module-object slots come first, in CPython's order, so
/// `list(vars(sys))` starts the way CPython's does.
const STARTUP_SLOTS: &[(&str, Slot)] = &[
    ("__name__", Slot::Eager(|| Value::string(MODULE_NAME))),
    ("__doc__", Slot::Eager(Value::none)),
    ("__package__", Slot::Eager(|| Value::string(""))),
    ("__loader__", Slot::Eager(Value::none)),
    ("__spec__", Slot::Eager(Value::none)),
    ("version", Slot::Eager(|| Value::string("PyRust 0.2"))),
    ("argv", Slot::Eager(|| Value::list(Vec::new()))),
    ("maxsize", Slot::Eager(|| Value::int(i64::MAX))),
    (
        "platform",
        Slot::Eager(|| Value::string(platform_info::sys_platform())),
    ),
    ("version_info", Slot::Deferred),
    ("stdout", Slot::Eager(pyrust_builtins::file::make_stdout)),
    ("stderr", Slot::Eager(pyrust_builtins::file::make_stderr)),
    ("stdin", Slot::Eager(pyrust_builtins::file::make_stdin)),
    ("path", PER_INTERPRETER),
    ("modules", PER_INTERPRETER),
    (
        "executable",
        Slot::Eager(|| Value::string(platform_info::sys_executable())),
    ),
    (
        "_base_executable",
        Slot::Eager(|| Value::string(platform_info::sys_executable())),
    ),
    ("_home", Slot::Eager(Value::none)),
    (
        "byteorder",
        Slot::Eager(|| Value::string(platform_info::sys_byteorder())),
    ),
    ("prefix", Slot::Eager(default_prefix)),
    ("exec_prefix", Slot::Eager(default_prefix)),
    ("base_prefix", Slot::Eager(default_prefix)),
    ("base_exec_prefix", Slot::Eager(default_prefix)),
    // 3.12.0 final → 0x030c00f0
    ("hexversion", Slot::Eager(|| Value::int(0x030c_00f0))),
    ("copyright", Slot::Eager(|| Value::string(PYRUST_COPYRIGHT))),
    #[cfg(windows)]
    ("winver", Slot::Eager(|| Value::string("3.12"))),
    ("flags", Slot::Deferred),
    ("maxunicode", Slot::Eager(|| Value::int(1_114_111))),
    ("abiflags", Slot::Eager(|| Value::string(""))),
    ("float_info", Slot::Deferred),
    ("int_info", Slot::Deferred),
    ("hash_info", Slot::Deferred),
    ("implementation", Slot::Deferred),
    ("builtin_module_names", Slot::Deferred),
    ("api_version", Slot::Eager(|| Value::int(1013))),
    ("dont_write_bytecode", PER_INTERPRETER),
    ("float_repr_style", Slot::Eager(|| Value::string("short"))),
    (
        "platlibdir",
        Slot::Eager(|| Value::string(platform_info::sys_platlibdir())),
    ),
    ("pycache_prefix", PER_INTERPRETER),
    ("orig_argv", PER_INTERPRETER),
    ("warnoptions", PER_INTERPRETER),
    ("meta_path", PER_INTERPRETER),
    ("path_hooks", PER_INTERPRETER),
    ("path_importer_cache", PER_INTERPRETER),
    ("_xoptions", PER_INTERPRETER),
    ("stdlib_module_names", Slot::Deferred),
    ("thread_info", Slot::Deferred),
    ("__stdout__", PER_INTERPRETER),
    ("__stderr__", PER_INTERPRETER),
    ("__stdin__", PER_INTERPRETER),
    ("__displayhook__", PER_INTERPRETER),
    ("__excepthook__", PER_INTERPRETER),
    ("__unraisablehook__", PER_INTERPRETER),
    ("__breakpointhook__", PER_INTERPRETER),
    ("monitoring", Slot::Deferred),
    #[cfg(windows)]
    (
        "dllhandle",
        Slot::Eager(|| Value::int(platform_info::sys_dllhandle())),
    ),
];

fn default_prefix() -> Value {
    Value::string(platform_info::default_sys_prefix())
}

// Thread-local justification:
// (a) Why `thread_local!`: test-only counter observing how many deferred
//     `sys` slots the current thread built.
// (b) Multi-interpreter / multi-threaded impact: none outside tests; each test
//     thread counts its own resolutions.
#[cfg(test)]
thread_local! {
    static DEFERRED_SLOT_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many deferred `sys` slots this thread has built so far.
#[cfg(test)]
pub(crate) fn deferred_slot_builds() -> usize {
    DEFERRED_SLOT_BUILDS.with(std::cell::Cell::get)
}

/// Build one deferred `sys` attribute by name.
///
/// Returns `None` without building anything for every other name.  The
/// owning module remembers the result (`per_module`), so this runs at most
/// once per slot and module object.
fn resolve_deferred(name: &str) -> Option<Value> {
    let value = match name {
        "version_info" => version_info::make_version_info(),
        // Replaced by the importing interpreter's configuration before the
        // module is published (`set_deferred_attr`); this default only serves
        // a `sys` module object that never went through an interpreter.
        "flags" => version_info::make_flags(),
        "float_info" => platform_info::make_float_info(),
        "int_info" => platform_info::make_int_info(),
        "hash_info" => platform_info::make_hash_info(),
        "implementation" => platform_info::make_implementation(),
        "builtin_module_names" => platform_info::make_builtin_module_names(),
        "stdlib_module_names" => platform_info::stdlib_module_names(),
        "thread_info" => platform_info::make_thread_info(),
        "monitoring" => monitoring::make_monitoring_module(),
        _ => return None,
    };
    #[cfg(test)]
    DEFERRED_SLOT_BUILDS.with(|count| count.set(count.get() + 1));
    Some(value)
}

/// Callables are bound after every slot above; the dotted registry keys
/// (`monitoring.use_tool_id`, `version_info.__ge__`, …) are method and
/// submodule dispatch entries, not Python-visible `sys` names.
fn for_each_callable(mut visit: impl FnMut(&'static str, &'static str)) {
    exports::for_each_function_export(|short_name, registry_name| {
        if !short_name.contains('.') {
            visit(short_name, registry_name);
        }
    });
}

fn visit_startup_slots(visit: LazyModuleSlotVisitor<'_>) {
    for (name, slot) in STARTUP_SLOTS {
        visit(name, matches!(slot, Slot::Deferred));
    }
    for_each_callable(|short_name, _| visit(short_name, false));
}

const DEFERRED_ATTRS: LazyModuleAttrSource = LazyModuleAttrSource {
    resolve: resolve_deferred,
    visit_slots: visit_startup_slots,
    per_module: true,
};

/// Sized for the eager slots plus the callables, to avoid a rehash while
/// populating.
const EAGER_SLOT_CAPACITY: usize = 128;

/// Build the `sys` module object.
///
/// Insertion-ordered (issue #2918): the slot table and the callable list are
/// fixed at compile time, so `vars(sys)` is stable across runs.
pub(crate) fn module() -> Value {
    let mut attrs = ModuleAttrs::with_capacity_and_hasher(EAGER_SLOT_CAPACITY, Default::default());
    for (name, slot) in STARTUP_SLOTS {
        if let Slot::Eager(build) = slot {
            attrs.insert((*name).to_string(), build());
        }
    }
    for_each_callable(|short_name, registry_name| {
        attrs.insert(
            short_name.to_string(),
            Value::fresh_builtin_function(registry_name),
        );
    });
    Value::py_module(Rc::new(RefCell::new(PyModule::with_lazy_attrs(
        MODULE_NAME.to_string(),
        attrs,
        DEFERRED_ATTRS,
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ValueKind;

    fn sys_module() -> Rc<RefCell<PyModule>> {
        match module().kind() {
            ValueKind::PyModule(module) => Rc::clone(module),
            _ => unreachable!("sys::module() builds a module"),
        }
    }

    const DEFERRED_NAMES: [&str; 10] = [
        "version_info",
        "flags",
        "float_info",
        "int_info",
        "hash_info",
        "implementation",
        "builtin_module_names",
        "stdlib_module_names",
        "thread_info",
        "monitoring",
    ];

    #[test]
    fn slot_table_and_resolver_agree_on_the_deferred_names() {
        let declared: Vec<&str> = STARTUP_SLOTS
            .iter()
            .filter(|(_, slot)| matches!(slot, Slot::Deferred))
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(declared, DEFERRED_NAMES);
        for (name, slot) in STARTUP_SLOTS {
            if matches!(slot, Slot::Eager(_)) {
                assert!(resolve_deferred(name).is_none(), "{name} is eager");
            }
        }
    }

    #[test]
    fn building_sys_allocates_no_deferred_slot() {
        let before = deferred_slot_builds();
        let module = sys_module();
        assert_eq!(deferred_slot_builds(), before);
        let module = module.borrow();
        assert!(module.has_pending_lazy_attrs());
        for name in DEFERRED_NAMES {
            assert!(
                !module.attrs.contains_key(name),
                "{name} must stay deferred"
            );
            assert!(module.deferred_attr_is_unresolved(name));
        }
        // Eager probes and misses build nothing either.
        assert!(module.get_attr_value("maxsize").is_some());
        assert!(module.get_attr_value("no_such_attribute").is_none());
        assert_eq!(deferred_slot_builds(), before);
    }

    #[test]
    fn a_deferred_slot_is_built_once_and_keeps_its_identity() {
        let module = sys_module();
        let before = deferred_slot_builds();
        let first = module.borrow().get_attr_value("monitoring").unwrap();
        let second = module.borrow().get_attr_value("monitoring").unwrap();
        assert!(crate::interpreter::values_are_identical(&first, &second));
        assert_eq!(deferred_slot_builds(), before + 1);
        // The other slots are still untouched.
        assert!(
            module
                .borrow()
                .deferred_attr_is_unresolved("stdlib_module_names")
        );
        // A second `sys` module object owns its own `monitoring` module.
        let other = sys_module().borrow().get_attr_value("monitoring").unwrap();
        assert!(!crate::interpreter::values_are_identical(&first, &other));
    }

    #[test]
    fn stdlib_module_names_is_shared_per_gil_group() {
        let first = sys_module()
            .borrow()
            .get_attr_value("stdlib_module_names")
            .unwrap();
        let second = sys_module()
            .borrow()
            .get_attr_value("stdlib_module_names")
            .unwrap();
        assert!(crate::interpreter::values_are_identical(&first, &second));
    }

    #[test]
    fn observing_the_namespace_keeps_startup_order() {
        let module = sys_module();
        let names: Vec<String> = module.borrow().attrs_snapshot().keys().cloned().collect();
        let mut expected: Vec<&str> = STARTUP_SLOTS.iter().map(|(name, _)| *name).collect();
        for_each_callable(|short_name, _| expected.push(short_name));
        assert_eq!(names, expected);
        assert!(names.iter().all(|name| !name.contains('.')));
    }
}
