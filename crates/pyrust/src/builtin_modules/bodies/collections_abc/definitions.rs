//! Canonical `collections.abc` `PyClass` singletons, required-method tables,
//! and out-of-band virtual subclass storage.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::IndexMap;

use crate::interpreter::{DictViewClass, class_is_subclass_of};
use crate::value::{PyClass, Value};

/// A virtual subclass registration: `registered` was virtually registered on
/// `abc` (matching CPython's `ABCMeta.register()`). Stored out-of-band so
/// registered ABCs NEVER appear in `registered.__bases__` or `__mro__`.
struct VirtualEntry {
    abc: Rc<RefCell<PyClass>>,
    registered: Weak<RefCell<PyClass>>,
}

macro_rules! abc_class {
    ($name:expr) => {{
        let mut attrs = IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("collections.abc"));
        Rc::new(RefCell::new(PyClass::new($name, $name, None, attrs)))
    }};
}

// Thread-local justification:
// (a) Why `thread_local!`: Stores `!Send` `Rc<RefCell<PyClass>>` singletons
//     for `collections.abc` abstract base classes.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread share the `collections.abc` class objects (e.g. `ABC_SEQUENCE`);
//     separate threads construct independent class singletons.
thread_local! {
    static ABC_CONTAINER:        Rc<RefCell<PyClass>> = abc_class!("Container");
    static ABC_HASHABLE:         Rc<RefCell<PyClass>> = abc_class!("Hashable");
    static ABC_ITERABLE:         Rc<RefCell<PyClass>> = abc_class!("Iterable");
    static ABC_ITERATOR:         Rc<RefCell<PyClass>> = abc_class!("Iterator");
    static ABC_REVERSIBLE:       Rc<RefCell<PyClass>> = abc_class!("Reversible");
    static ABC_GENERATOR:        Rc<RefCell<PyClass>> = abc_class!("Generator");
    static ABC_SIZED:            Rc<RefCell<PyClass>> = abc_class!("Sized");
    static ABC_COLLECTION:       Rc<RefCell<PyClass>> = abc_class!("Collection");
    static ABC_CALLABLE:         Rc<RefCell<PyClass>> = abc_class!("Callable");
    static ABC_SEQUENCE:         Rc<RefCell<PyClass>> = abc_class!("Sequence");
    static ABC_MUTABLE_SEQUENCE: Rc<RefCell<PyClass>> = abc_class!("MutableSequence");
    static ABC_SET:              Rc<RefCell<PyClass>> = abc_class!("Set");
    static ABC_MUTABLE_SET:      Rc<RefCell<PyClass>> = abc_class!("MutableSet");
    static ABC_MAPPING:          Rc<RefCell<PyClass>> = abc_class!("Mapping");
    static ABC_MUTABLE_MAPPING:  Rc<RefCell<PyClass>> = abc_class!("MutableMapping");
    static ABC_MAPPING_VIEW:     Rc<RefCell<PyClass>> = abc_class!("MappingView");
    static ABC_KEYS_VIEW:        Rc<RefCell<PyClass>> = abc_class!("KeysView");
    static ABC_ITEMS_VIEW:       Rc<RefCell<PyClass>> = abc_class!("ItemsView");
    static ABC_VALUES_VIEW:      Rc<RefCell<PyClass>> = abc_class!("ValuesView");
    static ABC_AWAITABLE:        Rc<RefCell<PyClass>> = abc_class!("Awaitable");
    static ABC_COROUTINE:        Rc<RefCell<PyClass>> = abc_class!("Coroutine");
    static ABC_ASYNC_ITERABLE:   Rc<RefCell<PyClass>> = abc_class!("AsyncIterable");
    static ABC_ASYNC_ITERATOR:   Rc<RefCell<PyClass>> = abc_class!("AsyncIterator");
    static ABC_ASYNC_GENERATOR:  Rc<RefCell<PyClass>> = abc_class!("AsyncGenerator");
    static ABC_BUFFER:           Rc<RefCell<PyClass>> = abc_class!("Buffer");

    // Once-per-thread flag: `true` after `init_abc_hierarchy_and_registry` has run.
    static ABC_REGISTERED: Cell<bool> = const { Cell::new(false) };

    // Once-per-thread flag: `true` after `collections_abc_py.py` mixins are installed.
    static ABC_MIXINS_INSTALLED: Cell<bool> = const { Cell::new(false) };

    // Out-of-band virtual subclass registrations (CPython `ABCMeta.register()`).
    static VIRTUAL_REGISTRATIONS: RefCell<Vec<VirtualEntry>> = const { RefCell::new(Vec::new()) };
}

/// Enumerates every canonical `collections.abc` singleton so module export,
/// hierarchy wiring, and mixin injection share a single source of truth.
#[derive(Clone, Copy)]
pub(super) enum Abc {
    Container,
    Hashable,
    Iterable,
    Iterator,
    Reversible,
    Generator,
    Sized,
    Collection,
    Callable,
    Sequence,
    MutableSequence,
    Set,
    MutableSet,
    Mapping,
    MutableMapping,
    MappingView,
    KeysView,
    ItemsView,
    ValuesView,
    Awaitable,
    Coroutine,
    AsyncIterable,
    AsyncIterator,
    AsyncGenerator,
    Buffer,
}

impl Abc {
    pub(super) const ALL: [Abc; 25] = [
        Self::Container,
        Self::Hashable,
        Self::Iterable,
        Self::Iterator,
        Self::Reversible,
        Self::Generator,
        Self::Sized,
        Self::Collection,
        Self::Callable,
        Self::Sequence,
        Self::MutableSequence,
        Self::Set,
        Self::MutableSet,
        Self::Mapping,
        Self::MutableMapping,
        Self::MappingView,
        Self::KeysView,
        Self::ItemsView,
        Self::ValuesView,
        Self::Awaitable,
        Self::Coroutine,
        Self::AsyncIterable,
        Self::AsyncIterator,
        Self::AsyncGenerator,
        Self::Buffer,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Container => "Container",
            Self::Hashable => "Hashable",
            Self::Iterable => "Iterable",
            Self::Iterator => "Iterator",
            Self::Reversible => "Reversible",
            Self::Generator => "Generator",
            Self::Sized => "Sized",
            Self::Collection => "Collection",
            Self::Callable => "Callable",
            Self::Sequence => "Sequence",
            Self::MutableSequence => "MutableSequence",
            Self::Set => "Set",
            Self::MutableSet => "MutableSet",
            Self::Mapping => "Mapping",
            Self::MutableMapping => "MutableMapping",
            Self::MappingView => "MappingView",
            Self::KeysView => "KeysView",
            Self::ItemsView => "ItemsView",
            Self::ValuesView => "ValuesView",
            Self::Awaitable => "Awaitable",
            Self::Coroutine => "Coroutine",
            Self::AsyncIterable => "AsyncIterable",
            Self::AsyncIterator => "AsyncIterator",
            Self::AsyncGenerator => "AsyncGenerator",
            Self::Buffer => "Buffer",
        }
    }

    pub(super) fn class(self) -> Rc<RefCell<PyClass>> {
        match self {
            Self::Container => ABC_CONTAINER.with(Rc::clone),
            Self::Hashable => ABC_HASHABLE.with(Rc::clone),
            Self::Iterable => ABC_ITERABLE.with(Rc::clone),
            Self::Iterator => ABC_ITERATOR.with(Rc::clone),
            Self::Reversible => ABC_REVERSIBLE.with(Rc::clone),
            Self::Generator => ABC_GENERATOR.with(Rc::clone),
            Self::Sized => ABC_SIZED.with(Rc::clone),
            Self::Collection => ABC_COLLECTION.with(Rc::clone),
            Self::Callable => ABC_CALLABLE.with(Rc::clone),
            Self::Sequence => ABC_SEQUENCE.with(Rc::clone),
            Self::MutableSequence => ABC_MUTABLE_SEQUENCE.with(Rc::clone),
            Self::Set => ABC_SET.with(Rc::clone),
            Self::MutableSet => ABC_MUTABLE_SET.with(Rc::clone),
            Self::Mapping => ABC_MAPPING.with(Rc::clone),
            Self::MutableMapping => ABC_MUTABLE_MAPPING.with(Rc::clone),
            Self::MappingView => ABC_MAPPING_VIEW.with(Rc::clone),
            Self::KeysView => ABC_KEYS_VIEW.with(Rc::clone),
            Self::ItemsView => ABC_ITEMS_VIEW.with(Rc::clone),
            Self::ValuesView => ABC_VALUES_VIEW.with(Rc::clone),
            Self::Awaitable => ABC_AWAITABLE.with(Rc::clone),
            Self::Coroutine => ABC_COROUTINE.with(Rc::clone),
            Self::AsyncIterable => ABC_ASYNC_ITERABLE.with(Rc::clone),
            Self::AsyncIterator => ABC_ASYNC_ITERATOR.with(Rc::clone),
            Self::AsyncGenerator => ABC_ASYNC_GENERATOR.with(Rc::clone),
            Self::Buffer => ABC_BUFFER.with(Rc::clone),
        }
    }
}

pub(super) fn is_registered() -> bool {
    ABC_REGISTERED.with(Cell::get)
}

pub(super) fn mark_registered() {
    ABC_REGISTERED.with(|f| f.set(true));
}

pub(super) fn are_mixins_installed() -> bool {
    ABC_MIXINS_INSTALLED.with(Cell::get)
}

pub(super) fn mark_mixins_installed() {
    ABC_MIXINS_INSTALLED.with(|f| f.set(true));
}

// ── Required-method table (mirrors CPython's __subclasshook__ impls) ──────────

const ABC_REQUIRED_METHODS: &[(&str, &[&str])] = &[
    ("Hashable", &["__hash__"]),
    ("Iterable", &["__iter__"]),
    ("Iterator", &["__iter__", "__next__"]),
    ("Reversible", &["__reversed__", "__iter__"]),
    (
        "Generator",
        &["__iter__", "__next__", "send", "throw", "close"],
    ),
    ("Sized", &["__len__"]),
    ("Container", &["__contains__"]),
    ("Collection", &["__len__", "__iter__", "__contains__"]),
    ("Callable", &["__call__"]),
    ("Buffer", &["__buffer__"]),
    ("Awaitable", &["__await__"]),
    ("Coroutine", &["__await__", "send", "throw", "close"]),
    ("AsyncIterable", &["__aiter__"]),
    ("AsyncIterator", &["__anext__", "__aiter__"]),
    (
        "AsyncGenerator",
        &["__aiter__", "__anext__", "asend", "athrow", "aclose"],
    ),
];

/// Required methods for an ABC, or `None` if the ABC has no structural hook.
pub(super) fn required_methods(abc_name: &str) -> Option<&'static [&'static str]> {
    ABC_REQUIRED_METHODS
        .iter()
        .find(|(name, _)| *name == abc_name)
        .map(|(_, methods)| *methods)
}

/// Return the immutable semantic name for one of the canonical structural ABCs.
pub(super) fn canonical_structural_abc_name(class: &Rc<RefCell<PyClass>>) -> Option<&'static str> {
    macro_rules! identify {
        ($($class_key:ident => $name:literal),+ $(,)?) => {
            $(
                if $class_key.with(|candidate| Rc::ptr_eq(class, candidate)) {
                    return Some($name);
                }
            )+
        };
    }
    identify! {
        ABC_HASHABLE => "Hashable",
        ABC_ITERABLE => "Iterable",
        ABC_ITERATOR => "Iterator",
        ABC_REVERSIBLE => "Reversible",
        ABC_GENERATOR => "Generator",
        ABC_SIZED => "Sized",
        ABC_CONTAINER => "Container",
        ABC_COLLECTION => "Collection",
        ABC_CALLABLE => "Callable",
        ABC_BUFFER => "Buffer",
        ABC_AWAITABLE => "Awaitable",
        ABC_COROUTINE => "Coroutine",
        ABC_ASYNC_ITERABLE => "AsyncIterable",
        ABC_ASYNC_ITERATOR => "AsyncIterator",
        ABC_ASYNC_GENERATOR => "AsyncGenerator",
    }
    None
}

/// CPython registers the dictionary-view families with the collections ABCs
/// virtually. Ordered views share their corresponding plain view's ABC shape.
pub(super) fn dict_view_is_virtual_abc(kind: DictViewClass, abc: &Rc<RefCell<PyClass>>) -> bool {
    let is = |candidate: &'static std::thread::LocalKey<Rc<RefCell<PyClass>>>| {
        candidate.with(|class| Rc::ptr_eq(class, abc))
    };
    let common = is(&ABC_MAPPING_VIEW)
        || is(&ABC_COLLECTION)
        || is(&ABC_ITERABLE)
        || is(&ABC_SIZED)
        || is(&ABC_CONTAINER)
        || is(&ABC_REVERSIBLE);
    match kind {
        DictViewClass::Keys | DictViewClass::OrderedKeys => {
            common || is(&ABC_KEYS_VIEW) || is(&ABC_SET)
        }
        DictViewClass::Items | DictViewClass::OrderedItems => {
            common || is(&ABC_ITEMS_VIEW) || is(&ABC_SET)
        }
        DictViewClass::Values | DictViewClass::OrderedValues => common || is(&ABC_VALUES_VIEW),
    }
}

pub(super) fn dict_view_instance_is_virtual_abc(value: &Value, abc: &Rc<RefCell<PyClass>>) -> bool {
    pyrust_builtins::dict_views::view_kind(value)
        .map(|kind| {
            DictViewClass::from_view(kind, pyrust_builtins::dict_views::is_ordered_view(value))
        })
        .is_some_and(|kind| dict_view_is_virtual_abc(kind, abc))
}

/// Register `registered` as a virtual subclass of `abc` without mutating
/// `registered.extra_bases` or `abc.subclasses`.
pub(super) fn register_virtual_subclass(
    abc: &Rc<RefCell<PyClass>>,
    registered: &Rc<RefCell<PyClass>>,
) {
    VIRTUAL_REGISTRATIONS.with(|regs| {
        let mut regs = regs.borrow_mut();
        regs.retain(|entry| entry.registered.strong_count() > 0);
        if regs.iter().any(|entry| {
            Rc::ptr_eq(&entry.abc, abc)
                && entry
                    .registered
                    .upgrade()
                    .is_some_and(|existing| Rc::ptr_eq(&existing, registered))
        }) {
            return;
        }
        regs.push(VirtualEntry {
            abc: Rc::clone(abc),
            registered: Rc::downgrade(registered),
        });
    });
}

/// True iff `cls` is a virtual subclass of `abc` via `VIRTUAL_REGISTRATIONS`.
pub(super) fn is_virtual_subclass(cls: &Rc<RefCell<PyClass>>, abc: &Rc<RefCell<PyClass>>) -> bool {
    VIRTUAL_REGISTRATIONS.with(|regs| {
        let mut regs = regs.borrow_mut();
        regs.retain(|entry| entry.registered.strong_count() > 0);
        regs.iter().any(|entry| {
            entry.registered.upgrade().is_some_and(|registered| {
                class_is_subclass_of(&entry.abc, abc) && class_is_subclass_of(cls, &registered)
            })
        })
    })
}
