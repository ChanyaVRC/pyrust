//! Canonical `collections.abc` class objects, the per-ABC metadata tables
//! (structural hook methods, abstract methods, docstrings), and the shared
//! storage both `collections.abc` and `_collections_abc` export.

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::{PyClass, Value};

pub(super) type ClassRc = Rc<RefCell<PyClass>>;

/// Every public `collections.abc` ABC, in CPython 3.12's definition order
/// (each ABC follows all of its bases).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Abc {
    Hashable,
    Awaitable,
    Coroutine,
    AsyncIterable,
    AsyncIterator,
    AsyncGenerator,
    Iterable,
    Iterator,
    Reversible,
    Generator,
    Sized,
    Container,
    Collection,
    Buffer,
    Callable,
    Set,
    MutableSet,
    Mapping,
    MappingView,
    KeysView,
    ItemsView,
    ValuesView,
    MutableMapping,
    Sequence,
    ByteString,
    MutableSequence,
}

impl Abc {
    /// Definition order (bases before subclasses).
    pub(super) const ALL: [Abc; 26] = [
        Self::Hashable,
        Self::Awaitable,
        Self::Coroutine,
        Self::AsyncIterable,
        Self::AsyncIterator,
        Self::AsyncGenerator,
        Self::Iterable,
        Self::Iterator,
        Self::Reversible,
        Self::Generator,
        Self::Sized,
        Self::Container,
        Self::Collection,
        Self::Buffer,
        Self::Callable,
        Self::Set,
        Self::MutableSet,
        Self::Mapping,
        Self::MappingView,
        Self::KeysView,
        Self::ItemsView,
        Self::ValuesView,
        Self::MutableMapping,
        Self::Sequence,
        Self::ByteString,
        Self::MutableSequence,
    ];

    /// CPython's `__all__`, in its source order.
    pub(super) const EXPORT_ORDER: [Abc; 26] = [
        Self::Awaitable,
        Self::Coroutine,
        Self::AsyncIterable,
        Self::AsyncIterator,
        Self::AsyncGenerator,
        Self::Hashable,
        Self::Iterable,
        Self::Iterator,
        Self::Generator,
        Self::Reversible,
        Self::Sized,
        Self::Container,
        Self::Callable,
        Self::Collection,
        Self::Set,
        Self::MutableSet,
        Self::Mapping,
        Self::MutableMapping,
        Self::MappingView,
        Self::KeysView,
        Self::ItemsView,
        Self::ValuesView,
        Self::Sequence,
        Self::MutableSequence,
        Self::ByteString,
        Self::Buffer,
    ];

    pub(super) const fn index(self) -> usize {
        self as usize
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Hashable => "Hashable",
            Self::Awaitable => "Awaitable",
            Self::Coroutine => "Coroutine",
            Self::AsyncIterable => "AsyncIterable",
            Self::AsyncIterator => "AsyncIterator",
            Self::AsyncGenerator => "AsyncGenerator",
            Self::Iterable => "Iterable",
            Self::Iterator => "Iterator",
            Self::Reversible => "Reversible",
            Self::Generator => "Generator",
            Self::Sized => "Sized",
            Self::Container => "Container",
            Self::Collection => "Collection",
            Self::Buffer => "Buffer",
            Self::Callable => "Callable",
            Self::Set => "Set",
            Self::MutableSet => "MutableSet",
            Self::Mapping => "Mapping",
            Self::MappingView => "MappingView",
            Self::KeysView => "KeysView",
            Self::ItemsView => "ItemsView",
            Self::ValuesView => "ValuesView",
            Self::MutableMapping => "MutableMapping",
            Self::Sequence => "Sequence",
            Self::ByteString => "ByteString",
            Self::MutableSequence => "MutableSequence",
        }
    }

    /// The methods `cls.__subclasshook__` checks with `_check_methods` when
    /// `cls` is exactly this ABC, or `None` when the ABC defines no hook of
    /// its own (it inherits one that answers `NotImplemented` for it).
    pub(super) const fn hook_methods(self) -> Option<&'static [&'static str]> {
        Some(match self {
            Self::Hashable => &["__hash__"],
            Self::Awaitable => &["__await__"],
            Self::Coroutine => &["__await__", "send", "throw", "close"],
            Self::AsyncIterable => &["__aiter__"],
            Self::AsyncIterator => &["__anext__", "__aiter__"],
            Self::AsyncGenerator => &["__aiter__", "__anext__", "asend", "athrow", "aclose"],
            Self::Iterable => &["__iter__"],
            Self::Iterator => &["__iter__", "__next__"],
            Self::Reversible => &["__reversed__", "__iter__"],
            Self::Generator => &["__iter__", "__next__", "send", "throw", "close"],
            Self::Sized => &["__len__"],
            Self::Container => &["__contains__"],
            Self::Collection => &["__len__", "__iter__", "__contains__"],
            Self::Buffer => &["__buffer__"],
            Self::Callable => &["__call__"],
            _ => return None,
        })
    }

    /// The `@abstractmethod`s this ABC defines in its own namespace.
    pub(super) const fn own_abstract_methods(self) -> &'static [&'static str] {
        match self {
            Self::Hashable => &["__hash__"],
            Self::Awaitable => &["__await__"],
            Self::Coroutine => &["send", "throw"],
            Self::AsyncIterable => &["__aiter__"],
            Self::AsyncIterator => &["__anext__"],
            Self::AsyncGenerator => &["asend", "athrow"],
            Self::Iterable => &["__iter__"],
            Self::Iterator => &["__next__"],
            Self::Reversible => &["__reversed__"],
            Self::Generator => &["send", "throw"],
            Self::Sized => &["__len__"],
            Self::Container => &["__contains__"],
            Self::Buffer => &["__buffer__"],
            Self::Callable => &["__call__"],
            Self::MutableSet => &["add", "discard"],
            Self::Mapping => &["__getitem__"],
            Self::MutableMapping => &["__setitem__", "__delitem__"],
            Self::Sequence => &["__getitem__"],
            Self::MutableSequence => &["__setitem__", "__delitem__", "insert"],
            _ => &[],
        }
    }

    /// The ABC's `__slots__` (all are `()` except `MappingView`).
    pub(super) const fn slots(self) -> &'static [&'static str] {
        match self {
            Self::MappingView => &["_mapping"],
            _ => &[],
        }
    }

    /// The class docstring, or `None` for the ABCs CPython leaves undocumented.
    pub(super) const fn doc(self) -> Option<&'static str> {
        match self {
            Self::Set => Some(SET_DOC),
            Self::MutableSet => Some(MUTABLE_SET_DOC),
            Self::Mapping => Some(MAPPING_DOC),
            Self::MutableMapping => Some(MUTABLE_MAPPING_DOC),
            Self::Sequence => Some(SEQUENCE_DOC),
            Self::ByteString => Some(BYTE_STRING_DOC),
            Self::MutableSequence => Some(MUTABLE_SEQUENCE_DOC),
            _ => None,
        }
    }

    pub(super) fn class(self) -> ClassRc {
        CANONICAL.with(|canonical| {
            let canonical = canonical.borrow();
            let canonical = canonical
                .as_ref()
                .expect("collections.abc classes are initialised before use");
            Rc::clone(&canonical.classes[self.index()])
        })
    }

    pub(super) fn value(self) -> Value {
        Value::py_class(self.class())
    }
}

pub(super) const MODULE_DOC: &str = "Abstract Base Classes (ABCs) for collections, according to PEP 3119.\n\n\
     Unit tests are in test_collections.\n";

const SET_DOC: &str = "A set is a finite, iterable container.\n\n    \
This class provides concrete generic implementations of all\n    \
methods except for __contains__, __iter__ and __len__.\n\n    \
To override the comparisons (presumably for speed, as the\n    \
semantics are fixed), redefine __le__ and __ge__,\n    \
then the other operations will automatically follow suit.\n    ";

const MUTABLE_SET_DOC: &str = "A mutable set is a finite, iterable container.\n\n    \
This class provides concrete generic implementations of all\n    \
methods except for __contains__, __iter__, __len__,\n    \
add(), and discard().\n\n    \
To override the comparisons (presumably for speed, as the\n    \
semantics are fixed), all you have to do is redefine __le__ and\n    \
then the other operations will automatically follow suit.\n    ";

const MAPPING_DOC: &str = "A Mapping is a generic container for associating key/value\n    \
pairs.\n\n    \
This class provides concrete generic implementations of all\n    \
methods except for __getitem__, __iter__, and __len__.\n    ";

const MUTABLE_MAPPING_DOC: &str = "A MutableMapping is a generic container for associating\n    \
key/value pairs.\n\n    \
This class provides concrete generic implementations of all\n    \
methods except for __getitem__, __setitem__, __delitem__,\n    \
__iter__, and __len__.\n    ";

const SEQUENCE_DOC: &str = "All the operations on a read-only sequence.\n\n    \
Concrete subclasses must override __new__ or __init__,\n    \
__getitem__, and __len__.\n    ";

const BYTE_STRING_DOC: &str = "This unifies bytes and bytearray.\n\n    \
XXX Should add all their methods.\n    ";

const MUTABLE_SEQUENCE_DOC: &str = "All the operations on a read-write sequence.\n\n    \
Concrete subclasses must provide __new__ or __init__,\n    \
__getitem__, __setitem__, __delitem__, __len__, and insert().\n    ";

/// The class objects and shared values every `collections.abc` /
/// `_collections_abc` module of this GIL group exports.
pub(super) struct Canonical {
    /// Indexed by [`Abc::index`].
    pub(super) classes: Vec<ClassRc>,
    /// The hidden `@abstractmethod` descriptor class and its bound form.
    pub(super) abstract_method: ClassRc,
    pub(super) bound_abstract_method: ClassRc,
    pub(super) callable_alias: ClassRc,
    pub(super) deprecate_meta: ClassRc,
    /// `MutableMapping._MutableMapping__marker`.
    pub(super) marker: Value,
    /// The `__all__` list both modules share.
    pub(super) all: Value,
    /// The private type objects `_collections_abc` exposes
    /// (`bytes_iterator`, `dict_keys`, `generator`, …), in CPython's order.
    pub(super) private_types: Vec<(&'static str, Value)>,
}

// Thread-local justification:
// (a) Why `thread_local!`: the canonical ABCs are `!Send` `Rc<RefCell<PyClass>>`
//     objects shared by `collections.abc`, `_collections_abc`, and the native
//     modules that register virtual subclasses on them.
// (b) Multi-interpreter / multi-threaded impact: interpreters on one GIL group
//     share one set of ABC objects (as the previous singletons did), so
//     `isinstance` checks agree across them; separate threads build their own.
pyrust_core::gil_local! {
    static CANONICAL: RefCell<Option<Rc<Canonical>>> = const { RefCell::new(None) };
}

pub(super) fn is_initialized() -> bool {
    CANONICAL.with(|canonical| canonical.borrow().is_some())
}

pub(super) fn install(canonical: Canonical) {
    CANONICAL.with(|slot| *slot.borrow_mut() = Some(Rc::new(canonical)));
}

/// The canonical state; only valid after initialisation.
pub(super) fn canonical() -> Rc<Canonical> {
    CANONICAL.with(|canonical| {
        Rc::clone(
            canonical
                .borrow()
                .as_ref()
                .expect("collections.abc classes are initialised before use"),
        )
    })
}

/// The ABC `class` is, by identity, if it is one of the canonical ABCs.
pub(super) fn abc_of(class: &ClassRc) -> Option<Abc> {
    CANONICAL.with(|canonical| {
        let canonical = canonical.borrow();
        let canonical = canonical.as_ref()?;
        Abc::ALL
            .into_iter()
            .find(|abc| Rc::ptr_eq(&canonical.classes[abc.index()], class))
    })
}
