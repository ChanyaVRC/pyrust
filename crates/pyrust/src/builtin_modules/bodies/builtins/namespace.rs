// Single-pass construction of the `builtins` namespace (issue #3294).
//
// One walk, `visit_startup_namespace`, describes every name `builtins` binds at
// startup, in its insertion order (issue #2918).  `module()` runs it once and
// stores only the cheap slots — registry function tokens and constants.  The
// class-valued slots (every built-in type and the whole exception hierarchy)
// are left to `PyModule`'s deferred-attribute backing, which runs the same
// walk again only when the complete namespace is actually observed
// (`vars(builtins)`, `dir(builtins)`, rebinding or deleting a class name).
//
// A script that never names an exception or a rare type therefore builds none
// of them, and no intermediate `PyModule` or attribute map exists at any point.

/// Names `builtins` binds to class objects rather than function tokens.
///
/// A name that a builtin family also registers as a constructor function keeps
/// that function's position in the namespace; the rest follow the constants in
/// this order.
const BUILTIN_TYPE_NAMES: [&str; 22] = [
    "bool",
    "bytearray",
    "bytes",
    "complex",
    "dict",
    "enumerate",
    "filter",
    "float",
    "frozenset",
    "int",
    "list",
    "map",
    "memoryview",
    "range",
    "reversed",
    "set",
    "slice",
    "str",
    "tuple",
    "zip",
    "type",
    "object",
];

/// Eagerly stored slots: flat functions that are not constructors, plus the
/// six namespace constants.  Sized to avoid a rehash while populating.
const EAGER_SLOT_CAPACITY: usize = 64;

/// What the startup namespace binds one name to.
enum BuiltinsSlot {
    /// Registry-dispatched function, identified by its registry name.
    Function(&'static str),
    /// Namespace data that is not a callable registry entry.
    Constant(Value),
    /// A type or exception class, resolved by `resolve_builtins_class`.
    Class,
}

#[inline]
fn builtin_type_slot(name: &str) -> Option<usize> {
    BUILTIN_TYPE_NAMES.iter().position(|known| *known == name)
}

/// Visit every name bound in `builtins` at startup, in insertion order.
fn visit_startup_namespace(visit: &mut dyn FnMut(&'static str, BuiltinsSlot)) {
    let mut placed_types = 0u32;
    let mut family_export = |short_name: &'static str, registry_name: &'static str| {
        // Dotted names (`list.__init__`, `BaseException.add_note`, …) are
        // method-descriptor registry keys.  They stay reachable through
        // `regs()` but are not Python-visible `builtins` names (issue #2971).
        if short_name.contains('.') {
            return;
        }
        match builtin_type_slot(short_name) {
            Some(slot) => {
                placed_types |= 1 << slot;
                visit(short_name, BuiltinsSlot::Class);
            }
            None => visit(short_name, BuiltinsSlot::Function(registry_name)),
        }
    };
    scalar::for_each_function_export(&mut family_export);
    identity_and_repr::for_each_function_export(&mut family_export);
    numeric_functions::for_each_function_export(&mut family_export);
    aggregation::for_each_function_export(&mut family_export);
    iteration::for_each_function_export(&mut family_export);
    reflection::for_each_function_export(&mut family_export);
    length::for_each_function_export(&mut family_export);
    ordering::for_each_function_export(&mut family_export);
    rounding::for_each_function_export(&mut family_export);
    sequence_constructors::for_each_function_export(&mut family_export);
    bytes_constructors::for_each_function_export(&mut family_export);
    complex_constructor::for_each_function_export(&mut family_export);
    collection_constructors::for_each_function_export(&mut family_export);
    text_constructor::for_each_function_export(&mut family_export);
    int_constructor::for_each_function_export(&mut family_export);
    float_constructor::for_each_function_export(&mut family_export);
    services::for_each_function_export(&mut family_export);
    container_protocols::for_each_function_export(&mut family_export);
    object_basics::for_each_function_export(&mut family_export);
    type_protocols::for_each_function_export(&mut family_export);
    primitive_construction_protocols::for_each_function_export(&mut family_export);
    object_comparison::for_each_function_export(&mut family_export);
    operators::for_each_function_export(&mut family_export);
    object_protocols::for_each_function_export(&mut family_export);
    exception_protocols::for_each_function_export(&mut family_export);
    importing::for_each_function_export(&mut family_export);
    typing_protocols::for_each_function_export(&mut family_export);

    // These objects are namespace data, not callable registry entries.  They
    // are part of the module at construction time so every builtins provider
    // (the canonical globals provider and a freshly imported module alike)
    // exposes the same authoritative mapping.
    visit("None", BuiltinsSlot::Constant(Value::none()));
    visit("True", BuiltinsSlot::Constant(Value::bool_(true)));
    visit("False", BuiltinsSlot::Constant(Value::bool_(false)));
    visit("Ellipsis", BuiltinsSlot::Constant(Value::ellipsis()));
    visit(
        "NotImplemented",
        BuiltinsSlot::Constant(Value::not_implemented()),
    );
    visit("__debug__", BuiltinsSlot::Constant(Value::bool_(true)));

    for (slot, name) in BUILTIN_TYPE_NAMES.into_iter().enumerate() {
        if placed_types & (1 << slot) == 0 {
            visit(name, BuiltinsSlot::Class);
        }
    }
    for name in crate::interpreter::BUILTINS_EXCEPTION_NAMES {
        visit(name, BuiltinsSlot::Class);
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: test-only counter observing how many deferred
//     class slots the current thread resolved through the builtins namespace.
// (b) Multi-interpreter / multi-threaded impact: none outside tests; each test
//     thread counts its own resolutions.
#[cfg(test)]
thread_local! {
    static DEFERRED_CLASS_RESOLUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many deferred class slots this thread has resolved so far.
#[cfg(test)]
pub(crate) fn deferred_class_resolutions() -> usize {
    DEFERRED_CLASS_RESOLUTIONS.with(std::cell::Cell::get)
}

/// Resolve one deferred `builtins` class by name.
///
/// Returns `None` without building anything for every other name, and always
/// hands back the same per-thread singleton, so a lookup needs no caching in
/// the module and never invalidates a `LoadGlobal` cache.  Resolving a type
/// leaves the exception hierarchy unbuilt, and vice versa.
fn resolve_builtins_class(name: &str) -> Option<Value> {
    let class = match name.as_bytes().first()? {
        b'a'..=b'z' => match name {
            "type" => type_class_singleton(),
            "object" => crate::interpreter::object_class_singleton(),
            _ => {
                builtin_type_slot(name)?;
                primitive_class_by_name(name)?
            }
        },
        b'A'..=b'Z' if crate::interpreter::BUILTINS_EXCEPTION_NAMES.contains(&name) => {
            crate::interpreter::lookup_exc_class(name)?
        }
        _ => return None,
    };
    #[cfg(test)]
    DEFERRED_CLASS_RESOLUTIONS.with(|count| count.set(count.get() + 1));
    Some(Value::py_class(class))
}

fn visit_startup_slots(visit: crate::value::LazyModuleSlotVisitor<'_>) {
    visit_startup_namespace(&mut |name, slot| match slot {
        BuiltinsSlot::Class => {
            if let Some(class) = resolve_builtins_class(name) {
                visit(name, Some(class));
            }
        }
        BuiltinsSlot::Function(_) | BuiltinsSlot::Constant(_) => visit(name, None),
    });
}

const DEFERRED_CLASSES: crate::value::LazyModuleAttrSource = crate::value::LazyModuleAttrSource {
    resolve: resolve_builtins_class,
    visit_slots: visit_startup_slots,
};

/// Build the `builtins` module in one pass over the builtin families.
///
/// Insertion-ordered (issue #2918): the family list and the name tables are
/// fixed at compile time, so `vars(builtins)` is stable across runs.
pub(crate) fn module() -> Value {
    let mut attrs =
        crate::value::ModuleAttrs::with_capacity_and_hasher(EAGER_SLOT_CAPACITY, Default::default());
    visit_startup_namespace(&mut |name, slot| {
        let value = match slot {
            BuiltinsSlot::Function(registry_name) => Value::builtin_function(registry_name),
            BuiltinsSlot::Constant(value) => value,
            BuiltinsSlot::Class => return,
        };
        attrs.insert(name.to_string(), value);
    });
    Value::py_module(Rc::new(RefCell::new(crate::value::PyModule::with_lazy_attrs(
        MODULE_NAME.to_string(),
        attrs,
        DEFERRED_CLASSES,
    ))))
}
