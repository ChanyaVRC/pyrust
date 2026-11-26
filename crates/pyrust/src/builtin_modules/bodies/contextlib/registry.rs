//! Generation-local class registry for `contextlib` internal helper classes.
//!
//! Receiver-less entry points (`contextmanager(func)`, `asynccontextmanager`)
//! and native coroutines have no instance to take their classes from, so the
//! registry resolves the newest imported generation by name. Entry points that
//! do have a receiver (a factory, an `ExitStack`, a `ContextDecorator`) use the
//! sibling pairs below so an object created by an older import generation
//! keeps producing helper objects of that same generation.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::interpreter::class_is_subclass_of;
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

const INTERNAL_CLASS_NAMES: [&str; 5] = [
    "_GeneratorContextManager",
    "_AsyncGeneratorContextManager",
    "_ContextManagerFactory",
    "_StackCallback",
    "_DecoratedCallable",
];

/// Helper classes that CPython's `contextlib` has no counterpart for. They are
/// detached from the module namespace once the module is built and kept alive
/// by the registry instead, so `dir(contextlib)` shows only CPython's names.
const HIDDEN_CLASS_NAMES: [&str; 4] = [
    "_AbstractMethod",
    "_ContextManagerFactory",
    "_DecoratedCallable",
    "_StackCallback",
];

/// Name of the hidden native coroutine class. CPython's `type(coro).__name__`
/// is `coroutine`, living in `builtins`, so it is detached from the module
/// namespace once the module is built.
const COROUTINE_CLASS_NAME: &str = "coroutine";

/// Which helper class a public/factory class produces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sibling {
    /// `_ContextManagerFactory` -> `_GeneratorContextManager`.
    GeneratorManager,
    /// `_ContextManagerFactory` -> `_AsyncGeneratorContextManager`.
    AsyncGeneratorManager,
    /// `ExitStack` / `AsyncExitStack` -> `_StackCallback`.
    StackCallback,
    /// `ContextDecorator` / `AsyncContextDecorator` -> `_DecoratedCallable`.
    DecoratedCallable,
}

/// `(sibling kind, key class name, sibling class name)` for every pair that is
/// registered when a module generation is prepared.
const SIBLING_PAIRS: [(Sibling, &str, &str); 6] = [
    (
        Sibling::GeneratorManager,
        "_ContextManagerFactory",
        "_GeneratorContextManager",
    ),
    (
        Sibling::AsyncGeneratorManager,
        "_ContextManagerFactory",
        "_AsyncGeneratorContextManager",
    ),
    (Sibling::StackCallback, "ExitStack", "_StackCallback"),
    (Sibling::StackCallback, "AsyncExitStack", "_StackCallback"),
    (
        Sibling::DecoratedCallable,
        "ContextDecorator",
        "_DecoratedCallable",
    ),
    (
        Sibling::DecoratedCallable,
        "AsyncContextDecorator",
        "_DecoratedCallable",
    ),
];

type WeakPyClass = Weak<RefCell<PyClass>>;
type ClassRc = Rc<RefCell<PyClass>>;
type InternalClassRegistry = RefCell<Vec<(&'static str, WeakPyClass)>>;

/// Pairs a weak public/factory class lifetime key with its strong private
/// sibling class so retained instances keep their generation's sibling alive
/// without leaking discarded imports after the key class is dropped.
struct ClassPairGeneration {
    kind: Sibling,
    key: WeakPyClass,
    sibling: ClassRc,
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

    /// Generation-local public/factory class -> helper class associations.
    /// Calling a retained factory must not switch to the newest imported
    /// contextlib generation.
    static CLASS_GENERATIONS:
        RefCell<Vec<ClassPairGeneration>> = const { RefCell::new(Vec::new()) };

    /// The hidden `coroutine` class of the newest prepared generation.
    static COROUTINE_CLASS: RefCell<Option<ClassRc>> = const { RefCell::new(None) };

    /// The `HIDDEN_CLASS_NAMES` classes of the newest prepared generation.
    static HIDDEN_CLASSES: RefCell<Vec<ClassRc>> = const { RefCell::new(Vec::new()) };
}

fn register_pair(kind: Sibling, key: &ClassRc, sibling: &ClassRc) {
    CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.key.strong_count() > 0);
        if generations
            .iter()
            .any(|generation| generation.kind == kind && generation.key.as_ptr() == Rc::as_ptr(key))
        {
            return;
        }
        generations.push(ClassPairGeneration {
            kind,
            key: Rc::downgrade(key),
            sibling: Rc::clone(sibling),
        });
    });
}

fn lookup_pair(kind: Sibling, key: &ClassRc) -> Option<ClassRc> {
    CLASS_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(|generation| generation.key.strong_count() > 0);
        generations.iter().rev().find_map(|generation| {
            if generation.kind != kind {
                return None;
            }
            let registered_key = generation.key.upgrade()?;
            class_is_subclass_of(key, &registered_key).then(|| Rc::clone(&generation.sibling))
        })
    })
}

/// Finalize and register the concrete classes exported by one imported
/// `contextlib` module generation.
pub(crate) fn register_module_generation(module: &Value) {
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
    let find = |wanted: &str| {
        classes
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, class)| class)
    };
    for (_, class) in &classes {
        class
            .borrow_mut()
            .attrs
            .insert("__module__".to_string(), Value::string("contextlib"));
    }
    for (kind, key_name, sibling_name) in SIBLING_PAIRS {
        if let (Some(key), Some(sibling)) = (find(key_name), find(sibling_name)) {
            register_pair(kind, key, sibling);
        }
    }
    INTERNAL_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|(_, class)| class.strong_count() > 0);
        for &name in &INTERNAL_CLASS_NAMES {
            let Some(class) = find(name) else {
                continue;
            };
            if !registry.iter().any(|(registered_name, registered)| {
                *registered_name == name && registered.as_ptr() == Rc::as_ptr(class)
            }) {
                registry.push((name, Rc::downgrade(class)));
            }
        }
    });
    let hidden: Vec<ClassRc> = HIDDEN_CLASS_NAMES
        .iter()
        .filter_map(|name| find(name).cloned())
        .collect();
    for name in HIDDEN_CLASS_NAMES {
        module.borrow_mut().attrs.shift_remove(name);
    }
    HIDDEN_CLASSES.with(|slot| *slot.borrow_mut() = hidden);
    if let Some(coroutine) = find(COROUTINE_CLASS_NAME).cloned() {
        {
            let mut class = coroutine.borrow_mut();
            class
                .attrs
                .insert("__module__".to_string(), Value::string("builtins"));
        }
        module.borrow_mut().attrs.shift_remove(COROUTINE_CLASS_NAME);
        COROUTINE_CLASS.with(|slot| *slot.borrow_mut() = Some(coroutine));
    }
}

fn registered_internal_class(name: &str) -> Option<ClassRc> {
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
fn module_class(name: &str) -> Option<ClassRc> {
    if let Some(class) = registered_internal_class(name) {
        return Some(class);
    }
    let module_value = super::module();
    super::prepare_module_generation(&module_value);
    registered_internal_class(name)
}

/// The hidden native `coroutine` class of the newest generation.
pub(super) fn coroutine_class() -> ClassRc {
    if let Some(class) = COROUTINE_CLASS.with(|slot| slot.borrow().clone()) {
        return class;
    }
    let module_value = super::module();
    super::prepare_module_generation(&module_value);
    COROUTINE_CLASS
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| unreachable!("internal: contextlib module did not register `coroutine`"))
}

fn build_instance(class: ClassRc, attrs: InstanceAttrs) -> Value {
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Construct a `PyInstance` of `name` with the given attrs, bypassing `__init__`.
pub(super) fn make_instance(name: &str, attrs: InstanceAttrs) -> Value {
    match module_class(name) {
        Some(class) => build_instance(class, attrs),
        None => unreachable!("internal: contextlib module did not register class `{name}`"),
    }
}

/// Construct the helper class instance owned by the generation of `key_class`
/// (falling back to the newest generation's `fallback_name` class).
pub(super) fn make_sibling_instance(
    kind: Sibling,
    key_class: &ClassRc,
    fallback_name: &str,
    attrs: InstanceAttrs,
) -> Value {
    let class = lookup_pair(kind, key_class)
        .or_else(|| module_class(fallback_name))
        .unwrap_or_else(|| {
            unreachable!("internal: contextlib module did not register class `{fallback_name}`")
        });
    build_instance(class, attrs)
}

#[cfg(test)]
mod generation_tests {
    use super::*;

    fn test_class(name: &str, base: Option<ClassRc>) -> ClassRc {
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
        register_pair(Sibling::GeneratorManager, &old_factory, &old_manager);

        let new_factory = test_class("new factory", None);
        let new_manager = test_class("new manager", None);
        register_pair(Sibling::GeneratorManager, &new_factory, &new_manager);

        let old_factory_subclass =
            test_class("old factory subclass", Some(Rc::clone(&old_factory)));
        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::GeneratorManager, &old_factory).unwrap(),
            &old_manager,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::GeneratorManager, &old_factory_subclass).unwrap(),
            &old_manager,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::GeneratorManager, &new_factory).unwrap(),
            &new_manager,
        ));
    }

    #[test]
    fn retained_exit_stack_resolves_callback_from_its_own_generation() {
        let old_stack = test_class("old ExitStack", None);
        let old_callback = test_class("old callback", None);
        register_pair(Sibling::StackCallback, &old_stack, &old_callback);

        let new_stack = test_class("new ExitStack", None);
        let new_callback = test_class("new callback", None);
        register_pair(Sibling::StackCallback, &new_stack, &new_callback);

        let old_stack_subclass = test_class("old stack subclass", Some(Rc::clone(&old_stack)));
        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::StackCallback, &old_stack).unwrap(),
            &old_callback,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::StackCallback, &old_stack_subclass).unwrap(),
            &old_callback,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::StackCallback, &new_stack).unwrap(),
            &new_callback,
        ));
    }

    #[test]
    fn sibling_kinds_do_not_cross_resolve_for_the_same_key() {
        let factory = test_class("factory", None);
        let sync_manager = test_class("sync manager", None);
        let async_manager = test_class("async manager", None);
        register_pair(Sibling::GeneratorManager, &factory, &sync_manager);
        register_pair(Sibling::AsyncGeneratorManager, &factory, &async_manager);

        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::GeneratorManager, &factory).unwrap(),
            &sync_manager,
        ));
        assert!(Rc::ptr_eq(
            &lookup_pair(Sibling::AsyncGeneratorManager, &factory).unwrap(),
            &async_manager,
        ));
    }
}
