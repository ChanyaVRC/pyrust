//! Generation-local class registry for `contextlib` internal helper classes.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::interpreter::class_is_subclass_of;
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

const INTERNAL_CLASS_NAMES: [&str; 3] = [
    "_GeneratorContextManager",
    "_ContextManagerFactory",
    "_StackCallback",
];

type WeakPyClass = Weak<RefCell<PyClass>>;
type InternalClassRegistry = RefCell<Vec<(&'static str, WeakPyClass)>>;

/// Pairs a weak public/factory class lifetime key with its strong private
/// sibling class so retained instances keep their generation's sibling alive
/// without leaking discarded imports after the key class is dropped.
struct ClassPairGeneration {
    key: WeakPyClass,
    sibling: Rc<RefCell<PyClass>>,
}

// Thread-local justification:
// (a) Why `thread_local!`: Stores `!Send` `Rc`/`Weak<RefCell<PyClass>>`
//     mappings pairing public `contextlib` classes with their internal helper
//     classes for each still-live import generation.
// (b) Multi-interpreter / multi-threaded impact: Weak generation tracking
//     isolates distinct `contextlib` imports across interpreters on one thread
//     and prunes dead generations; separate threads are completely isolated.
pyrust_core::gil_local! {
    /// Weak identities for every still-live imported contextlib generation.
    ///
    /// A fresh import after deleting `sys.modules["contextlib"]` must create a
    /// fresh set of classes. Old instances keep their generation alive through
    /// their strong class reference.
    static INTERNAL_CLASSES: InternalClassRegistry = const { RefCell::new(Vec::new()) };

    /// Generation-local `_ContextManagerFactory` -> `_GeneratorContextManager`
    /// associations. Calling a retained factory must not switch to the newest
    /// imported contextlib generation.
    static CONTEXT_MANAGER_CLASS_GENERATIONS:
        RefCell<Vec<ClassPairGeneration>> = const { RefCell::new(Vec::new()) };

    /// Generation-local `ExitStack` -> `_StackCallback` associations.
    static EXIT_STACK_CLASS_GENERATIONS:
        RefCell<Vec<ClassPairGeneration>> = const { RefCell::new(Vec::new()) };
}

fn register_pair(
    table: &'static pyrust_core::GilLocalKey<RefCell<Vec<ClassPairGeneration>>>,
    key: &Rc<RefCell<PyClass>>,
    sibling: &Rc<RefCell<PyClass>>,
) {
    table.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.key.strong_count() > 0);
        if generations
            .iter()
            .any(|generation| generation.key.as_ptr() == Rc::as_ptr(key))
        {
            return;
        }
        generations.push(ClassPairGeneration {
            key: Rc::downgrade(key),
            sibling: Rc::clone(sibling),
        });
    });
}

fn lookup_pair(
    table: &'static pyrust_core::GilLocalKey<RefCell<Vec<ClassPairGeneration>>>,
    key: &Rc<RefCell<PyClass>>,
) -> Option<Rc<RefCell<PyClass>>> {
    table.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.key.strong_count() > 0);
        generations.iter().rev().find_map(|generation| {
            let registered_key = generation.key.upgrade()?;
            class_is_subclass_of(key, &registered_key).then(|| Rc::clone(&generation.sibling))
        })
    })
}

/// Finalize and register the concrete classes exported by one imported
/// `contextlib` module generation.
pub(crate) fn prepare_module_classes(module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    let classes = module
        .borrow()
        .attrs
        .iter()
        .filter_map(|(name, value)| match value.kind() {
            ValueKind::PyClass(class) => Some((name.clone(), Rc::clone(class))),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (_, class) in &classes {
        class
            .borrow_mut()
            .attrs
            .insert("__module__".to_string(), Value::string("contextlib"));
    }
    if let (Some((_, factory)), Some((_, manager))) = (
        classes
            .iter()
            .find(|(name, _)| name == "_ContextManagerFactory"),
        classes
            .iter()
            .find(|(name, _)| name == "_GeneratorContextManager"),
    ) {
        register_pair(&CONTEXT_MANAGER_CLASS_GENERATIONS, factory, manager);
    }
    if let (Some((_, exit_stack)), Some((_, callback))) = (
        classes.iter().find(|(name, _)| name == "ExitStack"),
        classes.iter().find(|(name, _)| name == "_StackCallback"),
    ) {
        register_pair(&EXIT_STACK_CLASS_GENERATIONS, exit_stack, callback);
    }
    INTERNAL_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|(_, class)| class.strong_count() > 0);
        for &name in &INTERNAL_CLASS_NAMES {
            let Some((_, class)) = classes.iter().find(|(candidate, _)| candidate == name) else {
                continue;
            };
            if !registry.iter().any(|(registered_name, registered)| {
                *registered_name == name && registered.as_ptr() == Rc::as_ptr(class)
            }) {
                registry.push((name, Rc::downgrade(class)));
            }
        }
    });
}

fn registered_internal_class(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    INTERNAL_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|(_, class)| class.strong_count() > 0);
        registry
            .iter()
            .rev()
            .find(|(registered_name, _)| *registered_name == name)
            .and_then(|(_, class)| class.upgrade())
    })
}

/// Resolve the newest live imported generation for receiver-less bootstrap.
fn module_class(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    if let Some(class) = registered_internal_class(name) {
        return Some(class);
    }
    let module_value = super::module();
    prepare_module_classes(&module_value);
    registered_internal_class(name)
}

/// Construct a `PyInstance` of `name` with the given attrs, bypassing `__init__`.
fn make_instance(name: &str, attrs: InstanceAttrs) -> Value {
    match module_class(name) {
        Some(class) => Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs }))),
        None => unreachable!("internal: contextlib module did not register class `{name}`"),
    }
}

/// Construct a `_ContextManagerFactory` instance seeding `_func`.
pub(super) fn make_cm_factory(func: Value) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_func", func);
    make_instance("_ContextManagerFactory", attrs)
}

/// Construct the manager sibling owned by a concrete factory generation.
pub(super) fn make_generator_manager_instance(
    factory_class: &Rc<RefCell<PyClass>>,
    attrs: InstanceAttrs,
) -> Value {
    let class = lookup_pair(&CONTEXT_MANAGER_CLASS_GENERATIONS, factory_class)
        .or_else(|| module_class("_GeneratorContextManager"))
        .unwrap_or_else(|| {
            unreachable!(
                "internal: contextlib module did not register class `_GeneratorContextManager`"
            )
        });
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Construct the callback sibling owned by a concrete ExitStack generation.
pub(super) fn make_stack_callback_instance(
    exit_stack_class: &Rc<RefCell<PyClass>>,
    attrs: InstanceAttrs,
) -> Value {
    let class = lookup_pair(&EXIT_STACK_CLASS_GENERATIONS, exit_stack_class)
        .or_else(|| module_class("_StackCallback"))
        .unwrap_or_else(|| {
            unreachable!("internal: contextlib module did not register class `_StackCallback`")
        });
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

#[cfg(test)]
mod generation_tests {
    use super::*;

    fn test_class(name: &str, base: Option<Rc<RefCell<PyClass>>>) -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            qualname: name.to_string(),
            base,
            ..PyClass::default()
        }))
    }

    #[test]
    fn retained_factory_resolves_manager_from_its_own_generation() {
        let old_factory = test_class("old factory", None);
        let old_manager = test_class("old manager", None);
        register_pair(
            &CONTEXT_MANAGER_CLASS_GENERATIONS,
            &old_factory,
            &old_manager,
        );

        let new_factory = test_class("new factory", None);
        let new_manager = test_class("new manager", None);
        register_pair(
            &CONTEXT_MANAGER_CLASS_GENERATIONS,
            &new_factory,
            &new_manager,
        );

        let old_factory_subclass =
            test_class("old factory subclass", Some(Rc::clone(&old_factory)));
        assert!(Rc::ptr_eq(
            &lookup_pair(&CONTEXT_MANAGER_CLASS_GENERATIONS, &old_factory).unwrap(),
            &old_manager,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(&CONTEXT_MANAGER_CLASS_GENERATIONS, &old_factory_subclass).unwrap(),
            &old_manager,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(&CONTEXT_MANAGER_CLASS_GENERATIONS, &new_factory).unwrap(),
            &new_manager,
        ));
    }

    #[test]
    fn retained_exit_stack_resolves_callback_from_its_own_generation() {
        let old_stack = test_class("old ExitStack", None);
        let old_callback = test_class("old callback", None);
        register_pair(&EXIT_STACK_CLASS_GENERATIONS, &old_stack, &old_callback);

        let new_stack = test_class("new ExitStack", None);
        let new_callback = test_class("new callback", None);
        register_pair(&EXIT_STACK_CLASS_GENERATIONS, &new_stack, &new_callback);

        let old_stack_subclass = test_class("old stack subclass", Some(Rc::clone(&old_stack)));
        assert!(Rc::ptr_eq(
            &lookup_pair(&EXIT_STACK_CLASS_GENERATIONS, &old_stack).unwrap(),
            &old_callback,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(&EXIT_STACK_CLASS_GENERATIONS, &old_stack_subclass).unwrap(),
            &old_callback,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(&EXIT_STACK_CLASS_GENERATIONS, &new_stack).unwrap(),
            &new_callback,
        ));
    }
}
