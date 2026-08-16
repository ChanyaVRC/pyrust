//! Module-generation state and synthetic `typing` class ownership.
//!
//! This module is deliberately interpreter-free. It owns the weak registries
//! that define which classes belong to one imported `typing` generation, while
//! callers provide the builders for alias-specific class shapes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::value::{PyClass, PyInstance, Value};
use indexmap::IndexMap;

/// (method-short, registry-name) pairs for `_Any`.
const ANY_METHODS: &[(&str, &str)] = &[
    ("__repr__", "typing._Any.__repr__"),
    ("__init__", "typing._Any.__init__"),
];

/// (method-short, registry-name) pairs for `_GenericAlias`.
const GENERIC_ALIAS_METHODS: &[(&str, &str)] = &[
    ("__repr__", "typing._GenericAlias.__repr__"),
    ("__init__", "typing._GenericAlias.__init__"),
    ("__call__", "typing._GenericAlias.__call__"),
    ("__mro_entries__", "typing._GenericAlias.__mro_entries__"),
];

/// (method-short, registry-name) pairs for `_TypingAlias`.
const TYPING_ALIAS_METHODS: &[(&str, &str)] = &[
    ("__repr__", "typing._TypingAlias.__repr__"),
    ("__init__", "typing._TypingAlias.__init__"),
    ("__class_getitem__", "typing._TypingAlias.__class_getitem__"),
];

// `typing.Generic` is backed by `_typing` and remains identical across a
// `typing` re-import in CPython 3.12. Keep that one class process-stable while
// every Python-owned class below is generation-local.
thread_local! {
    static STABLE_GENERIC_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert(
            "__class_getitem__".to_string(),
            Value::builtin_function("typing._generic_cgi"),
        );
        attrs.insert("__module__".to_string(), Value::string("typing"));
        Rc::new(RefCell::new(PyClass::new("Generic", "Generic", None, attrs)))
    };
}

#[derive(Default)]
struct TypingGeneration {
    any_class: Weak<RefCell<PyClass>>,
    typing_alias_class: Weak<RefCell<PyClass>>,
    generic_alias_class: Weak<RefCell<PyClass>>,
    generic_class: Weak<RefCell<PyClass>>,
    protocol_class: Weak<RefCell<PyClass>>,
    namedtuple_marker_class: Weak<RefCell<PyClass>>,
    typeddict_marker_class: Weak<RefCell<PyClass>>,
    annotated_marker: Weak<RefCell<PyInstance>>,
    special_forms: HashMap<&'static str, Weak<RefCell<PyClass>>>,
    legacy_aliases: HashMap<&'static str, Weak<RefCell<PyClass>>>,
}

impl TypingGeneration {
    fn has_live_owned_value(&self) -> bool {
        self.any_class.strong_count() > 0
            || self.typing_alias_class.strong_count() > 0
            || self.generic_alias_class.strong_count() > 0
            || self.protocol_class.strong_count() > 0
            || self.namedtuple_marker_class.strong_count() > 0
            || self.typeddict_marker_class.strong_count() > 0
            || self.annotated_marker.strong_count() > 0
            || self
                .special_forms
                .values()
                .any(|class| class.strong_count() > 0)
            || self
                .legacy_aliases
                .values()
                .any(|class| class.strong_count() > 0)
    }
}

thread_local! {
    static TYPING_GENERATIONS: RefCell<Vec<TypingGeneration>> =
        const { RefCell::new(Vec::new()) };
}

pub(super) fn stable_generic_class() -> Rc<RefCell<PyClass>> {
    STABLE_GENERIC_CLASS.with(Rc::clone)
}

pub(super) fn start_typing_generation() {
    let generic = stable_generic_class();
    TYPING_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        generations.retain(TypingGeneration::has_live_owned_value);
        generations.push(TypingGeneration {
            generic_class: Rc::downgrade(&generic),
            ..TypingGeneration::default()
        });
    });
}

fn ensure_typing_generation() {
    let empty = TYPING_GENERATIONS.with(|generations| generations.borrow().is_empty());
    if empty {
        start_typing_generation();
    }
}

fn current_generation_class(
    lookup: impl Fn(&TypingGeneration) -> Option<Rc<RefCell<PyClass>>>,
    store: impl FnOnce(&mut TypingGeneration, Weak<RefCell<PyClass>>),
    build: impl FnOnce() -> Rc<RefCell<PyClass>>,
) -> Rc<RefCell<PyClass>> {
    ensure_typing_generation();
    if let Some(class) =
        TYPING_GENERATIONS.with(|generations| generations.borrow().last().and_then(&lookup))
    {
        return class;
    }
    let class = build();
    TYPING_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        let generation = generations
            .last_mut()
            .expect("typing generation was ensured before class construction");
        store(generation, Rc::downgrade(&class));
    });
    class
}

fn build_method_class(
    name: &'static str,
    methods: &'static [(&'static str, &'static str)],
) -> Rc<RefCell<PyClass>> {
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    for (method, reg_name) in methods {
        attrs.insert((*method).to_string(), Value::builtin_function(reg_name));
    }
    attrs.insert("__module__".to_string(), Value::string("typing"));
    Rc::new(RefCell::new(PyClass::new(name, name, None, attrs)))
}

pub(super) fn current_any_class() -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.any_class.upgrade(),
        |generation, class| generation.any_class = class,
        || build_method_class("_Any", ANY_METHODS),
    )
}

pub(super) fn current_typing_alias_class() -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.typing_alias_class.upgrade(),
        |generation, class| generation.typing_alias_class = class,
        || build_method_class("_TypingAlias", TYPING_ALIAS_METHODS),
    )
}

pub(super) fn current_generic_alias_class() -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.generic_alias_class.upgrade(),
        |generation, class| generation.generic_alias_class = class,
        || build_method_class("_GenericAlias", GENERIC_ALIAS_METHODS),
    )
}

pub(super) fn current_generic_class() -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.generic_class.upgrade(),
        |generation, class| generation.generic_class = class,
        stable_generic_class,
    )
}

fn build_protocol_class() -> Rc<RefCell<PyClass>> {
    let generic = current_generic_class();
    let mut attrs: IndexMap<String, Value> = IndexMap::new();
    attrs.insert(
        "__class_getitem__".to_string(),
        Value::builtin_function("typing._protocol_cgi"),
    );
    attrs.insert("__module__".to_string(), Value::string("typing"));
    let protocol = Rc::new(RefCell::new(PyClass::new(
        "Protocol",
        "Protocol",
        Some(Rc::clone(&generic)),
        attrs,
    )));
    generic
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&protocol));
    protocol
}

pub(super) fn current_protocol_class() -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.protocol_class.upgrade(),
        |generation, class| generation.protocol_class = class,
        build_protocol_class,
    )
}

fn build_typing_marker(
    name: &'static str,
    kind: pyrust_builtins::typing_marker::TypingMarkerKind,
) -> Rc<RefCell<PyClass>> {
    let class = build_method_class(name, &[]);
    pyrust_builtins::typing_marker::register(&class, kind);
    class
}

pub(super) fn current_namedtuple_marker_class() -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.namedtuple_marker_class.upgrade(),
        |generation, class| generation.namedtuple_marker_class = class,
        || {
            build_typing_marker(
                "NamedTuple",
                pyrust_builtins::typing_marker::TypingMarkerKind::NamedTuple,
            )
        },
    )
}

pub(super) fn current_typeddict_marker_class() -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.typeddict_marker_class.upgrade(),
        |generation, class| generation.typeddict_marker_class = class,
        || {
            build_typing_marker(
                "TypedDict",
                pyrust_builtins::typing_marker::TypingMarkerKind::TypedDict,
            )
        },
    )
}

pub(super) fn protocol_classes() -> Vec<Rc<RefCell<PyClass>>> {
    TYPING_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        let classes = generations
            .iter()
            .filter_map(|generation| generation.protocol_class.upgrade())
            .collect();
        generations.retain(TypingGeneration::has_live_owned_value);
        classes
    })
}

/// True if `class` belongs to any live `typing.Protocol` generation.
pub(crate) fn is_protocol_subclass(class: &Rc<RefCell<PyClass>>) -> bool {
    protocol_classes()
        .iter()
        .any(|protocol| crate::interpreter::class_is_subclass_of(class, protocol))
}

/// True if `class` is a bare marker from any live Protocol generation.
pub(crate) fn is_protocol_marker_class(class: &Rc<RefCell<PyClass>>) -> bool {
    protocol_classes()
        .iter()
        .any(|protocol| Rc::ptr_eq(class, protocol))
}

pub(super) fn current_special_form_class(
    name: &'static str,
    build: impl FnOnce() -> Rc<RefCell<PyClass>>,
) -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.special_forms.get(name).and_then(Weak::upgrade),
        |generation, class| {
            generation.special_forms.insert(name, class);
        },
        build,
    )
}

/// True when `class` is a subscriptable special form from any live import.
pub(super) fn is_special_form_class(class: &Rc<RefCell<PyClass>>) -> bool {
    TYPING_GENERATIONS.with(|generations| {
        generations.borrow().iter().any(|generation| {
            generation
                .special_forms
                .values()
                .filter_map(Weak::upgrade)
                .any(|registered| Rc::ptr_eq(&registered, class))
        })
    })
}

/// Record the Python-owned bare `typing.Annotated` marker for this import.
pub(super) fn register_annotated_marker(value: &Value) {
    let Some(instance) = value.as_py_instance_rc() else {
        return;
    };
    ensure_typing_generation();
    TYPING_GENERATIONS.with(|generations| {
        let mut generations = generations.borrow_mut();
        let generation = generations
            .last_mut()
            .expect("typing generation was ensured before Annotated injection");
        generation.annotated_marker = Rc::downgrade(instance);
    });
}

/// True when `value` is bare `typing.Annotated` from any live import.
pub(super) fn is_annotated_marker(value: &Value) -> bool {
    let Some(instance) = value.as_py_instance_rc() else {
        return false;
    };
    TYPING_GENERATIONS.with(|generations| {
        generations.borrow().iter().any(|generation| {
            generation
                .annotated_marker
                .upgrade()
                .is_some_and(|registered| Rc::ptr_eq(&registered, instance))
        })
    })
}

pub(super) fn paired_special_form_class(
    receiver: &Value,
    target: &str,
) -> Option<Rc<RefCell<PyClass>>> {
    let crate::value::ValueKind::PyClass(receiver_class) = receiver.kind() else {
        return None;
    };
    TYPING_GENERATIONS.with(|generations| {
        generations.borrow().iter().find_map(|generation| {
            let owns_receiver = generation
                .special_forms
                .values()
                .filter_map(Weak::upgrade)
                .any(|class| Rc::ptr_eq(&class, receiver_class));
            if owns_receiver {
                generation.special_forms.get(target).and_then(Weak::upgrade)
            } else {
                None
            }
        })
    })
}

pub(super) fn is_union_class(value: &Value) -> bool {
    let crate::value::ValueKind::PyClass(class) = value.kind() else {
        return false;
    };
    TYPING_GENERATIONS.with(|generations| {
        generations.borrow().iter().any(|generation| {
            generation
                .special_forms
                .get("Union")
                .and_then(Weak::upgrade)
                .is_some_and(|union| Rc::ptr_eq(class, &union))
        })
    })
}

pub(super) fn current_legacy_alias_class(
    name: &'static str,
    build: impl FnOnce() -> Rc<RefCell<PyClass>>,
) -> Rc<RefCell<PyClass>> {
    current_generation_class(
        |generation| generation.legacy_aliases.get(name).and_then(Weak::upgrade),
        |generation, class| {
            generation.legacy_aliases.insert(name, class);
        },
        build,
    )
}

pub(super) fn is_legacy_alias_class(class: &Rc<RefCell<PyClass>>) -> bool {
    TYPING_GENERATIONS.with(|generations| {
        generations.borrow().iter().any(|generation| {
            generation
                .legacy_aliases
                .values()
                .filter_map(Weak::upgrade)
                .any(|registered| Rc::ptr_eq(&registered, class))
        })
    })
}
