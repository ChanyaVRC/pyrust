/// Holder for the per-primitive `PyClass` Rc's.  Constructed once per
/// thread at startup, then cloned cheaply (Rc::clone) on every `type(x)` /
/// `resolve_builtin("int")` etc. call.
pub(crate) struct PrimitiveClasses {
    pub(crate) bool_class: Rc<RefCell<PyClass>>,
    pub(crate) bytearray_class: Rc<RefCell<PyClass>>,
    pub(crate) bytes_class: Rc<RefCell<PyClass>>,
    pub(crate) complex_class: Rc<RefCell<PyClass>>,
    pub(crate) dict_class: Rc<RefCell<PyClass>>,
    pub(crate) ellipsis_class: Rc<RefCell<PyClass>>,
    pub(crate) float_class: Rc<RefCell<PyClass>>,
    pub(crate) frozenset_class: Rc<RefCell<PyClass>>,
    pub(crate) int_class: Rc<RefCell<PyClass>>,
    pub(crate) list_class: Rc<RefCell<PyClass>>,
    pub(crate) mappingproxy_class: Rc<RefCell<PyClass>>,
    pub(crate) none_class: Rc<RefCell<PyClass>>,
    pub(crate) notimplemented_class: Rc<RefCell<PyClass>>,
    pub(crate) set_class: Rc<RefCell<PyClass>>,
    pub(crate) str_class: Rc<RefCell<PyClass>>,
    pub(crate) tuple_class: Rc<RefCell<PyClass>>,
}

/// Runtime-local spelling for the cross-crate canonical class tag. Primitive
/// layout/slot code consumes only tags for which `is_primitive()` is true.
pub(crate) type PrimitiveClassKind = pyrust_core::CanonicalClassTag;

/// The `builtins` types that are real classes in CPython but carry no
/// primitive storage variant of their own — they wrap an iterator state or an
/// opaque built-in object instead (issue #3000).
///
/// `range` is deliberately absent: it predates this group and owns a separate
/// singleton with slot-dunder attributes ([`range_class_singleton`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BuiltinTypeClass {
    Zip,
    Map,
    Filter,
    Enumerate,
    Slice,
    Reversed,
}

/// The six opaque dictionary-view classes in CPython: three plain views and
/// their three OrderedDict-specific final subclasses. Unlike
/// [`BuiltinTypeClass`], none of these names is bound in the `builtins` module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DictViewClass {
    Keys,
    Items,
    Values,
    OrderedKeys,
    OrderedItems,
    OrderedValues,
}

/// Concrete iterator classes whose public identity differs from their shared
/// generator-backed storage. CPython publishes the forward deque iterator as
/// `collections._deque_iterator`; the other two are discovered through
/// `type(iterator)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NativeIteratorClass {
    Bytearray,
    Deque,
    DequeReverse,
}

impl NativeIteratorClass {
    pub(crate) const ALL: [Self; 3] = [Self::Bytearray, Self::Deque, Self::DequeReverse];

    pub(crate) const fn full_type_name(self) -> &'static str {
        match self {
            Self::Bytearray => "bytearray_iterator",
            Self::Deque => "collections._deque_iterator",
            Self::DequeReverse => "collections._deque_reverse_iterator",
        }
    }

    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::Bytearray => "bytearray_iterator",
            Self::Deque => "_deque_iterator",
            Self::DequeReverse => "_deque_reverse_iterator",
        }
    }

    pub(crate) const fn module(self) -> &'static str {
        match self {
            Self::Bytearray => "builtins",
            Self::Deque | Self::DequeReverse => "collections",
        }
    }

    pub(crate) fn singleton(self) -> Rc<RefCell<PyClass>> {
        NATIVE_ITERATOR_CLASSES.with(|classes| Rc::clone(&classes[self as usize]))
    }

    pub(crate) fn from_class(class: &Rc<RefCell<PyClass>>) -> Option<Self> {
        let ptr = Rc::as_ptr(class);
        NATIVE_ITERATOR_CLASSES.with(|classes| {
            Self::ALL
                .into_iter()
                .zip(classes)
                .find_map(|(kind, candidate)| (ptr == Rc::as_ptr(candidate)).then_some(kind))
        })
    }
}

impl DictViewClass {
    pub(crate) const ALL: [Self; 6] = [
        Self::Keys,
        Self::Items,
        Self::Values,
        Self::OrderedKeys,
        Self::OrderedItems,
        Self::OrderedValues,
    ];

    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::Keys => pyrust_builtins::dict_views::DICT_KEYS_TYPE_NAME,
            Self::Items => pyrust_builtins::dict_views::DICT_ITEMS_TYPE_NAME,
            Self::Values => pyrust_builtins::dict_views::DICT_VALUES_TYPE_NAME,
            Self::OrderedKeys => pyrust_builtins::dict_views::ODICT_KEYS_TYPE_NAME,
            Self::OrderedItems => pyrust_builtins::dict_views::ODICT_ITEMS_TYPE_NAME,
            Self::OrderedValues => pyrust_builtins::dict_views::ODICT_VALUES_TYPE_NAME,
        }
    }

    pub(crate) const fn is_set_like(self) -> bool {
        matches!(
            self,
            Self::Keys | Self::Items | Self::OrderedKeys | Self::OrderedItems
        )
    }

    pub(crate) const fn is_ordered(self) -> bool {
        matches!(
            self,
            Self::OrderedKeys | Self::OrderedItems | Self::OrderedValues
        )
    }

    pub(crate) const fn view_kind(self) -> pyrust_builtins::dict_views::DictViewKind {
        match self {
            Self::Keys | Self::OrderedKeys => pyrust_builtins::dict_views::DictViewKind::Keys,
            Self::Items | Self::OrderedItems => pyrust_builtins::dict_views::DictViewKind::Items,
            Self::Values | Self::OrderedValues => pyrust_builtins::dict_views::DictViewKind::Values,
        }
    }

    pub(crate) const fn from_view(
        kind: pyrust_builtins::dict_views::DictViewKind,
        ordered: bool,
    ) -> Self {
        match (kind, ordered) {
            (pyrust_builtins::dict_views::DictViewKind::Keys, false) => Self::Keys,
            (pyrust_builtins::dict_views::DictViewKind::Items, false) => Self::Items,
            (pyrust_builtins::dict_views::DictViewKind::Values, false) => Self::Values,
            (pyrust_builtins::dict_views::DictViewKind::Keys, true) => Self::OrderedKeys,
            (pyrust_builtins::dict_views::DictViewKind::Items, true) => Self::OrderedItems,
            (pyrust_builtins::dict_views::DictViewKind::Values, true) => Self::OrderedValues,
        }
    }

    pub(crate) fn from_class_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.class_name() == name)
    }

    pub(crate) fn from_class(class: &Rc<RefCell<PyClass>>) -> Option<Self> {
        let ptr = Rc::as_ptr(class);
        DICT_VIEW_CLASSES.with(|classes| {
            Self::ALL
                .into_iter()
                .zip(classes)
                .find_map(|(kind, candidate)| (ptr == Rc::as_ptr(candidate)).then_some(kind))
        })
    }

    pub(crate) fn singleton(self) -> Rc<RefCell<PyClass>> {
        DICT_VIEW_CLASSES.with(|classes| Rc::clone(&classes[self as usize]))
    }
}

impl BuiltinTypeClass {
    /// Every member, in the order the per-thread singleton array stores them.
    pub(crate) const ALL: [Self; 6] = [
        Self::Zip,
        Self::Map,
        Self::Filter,
        Self::Enumerate,
        Self::Slice,
        Self::Reversed,
    ];

    /// Members whose native iterator layout may be inherited by a Python
    /// subclass.  `slice` is deliberately absent because CPython does not set
    /// `Py_TPFLAGS_BASETYPE` on it.
    pub(crate) const SUBCLASSABLE: [Self; 5] = [
        Self::Zip,
        Self::Map,
        Self::Filter,
        Self::Enumerate,
        Self::Reversed,
    ];

    /// The Python-visible class name, which is also this type's `builtins`
    /// binding and its constructor's key in the builtin registry.
    pub(crate) const fn class_name(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Map => "map",
            Self::Filter => "filter",
            Self::Enumerate => "enumerate",
            Self::Slice => "slice",
            Self::Reversed => "reversed",
        }
    }

    pub(crate) const fn tag(self) -> pyrust_core::BuiltinTypeClassTag {
        match self {
            Self::Zip => pyrust_core::BuiltinTypeClassTag::Zip,
            Self::Map => pyrust_core::BuiltinTypeClassTag::Map,
            Self::Filter => pyrust_core::BuiltinTypeClassTag::Filter,
            Self::Enumerate => pyrust_core::BuiltinTypeClassTag::Enumerate,
            Self::Slice => pyrust_core::BuiltinTypeClassTag::Slice,
            Self::Reversed => pyrust_core::BuiltinTypeClassTag::Reversed,
        }
    }

    pub(crate) const fn from_tag(tag: pyrust_core::BuiltinTypeClassTag) -> Self {
        match tag {
            pyrust_core::BuiltinTypeClassTag::Zip => Self::Zip,
            pyrust_core::BuiltinTypeClassTag::Map => Self::Map,
            pyrust_core::BuiltinTypeClassTag::Filter => Self::Filter,
            pyrust_core::BuiltinTypeClassTag::Enumerate => Self::Enumerate,
            pyrust_core::BuiltinTypeClassTag::Slice => Self::Slice,
            pyrust_core::BuiltinTypeClassTag::Reversed => Self::Reversed,
        }
    }

    /// CPython gives `zip` / `map` / `filter` / `enumerate` / `reversed` the
    /// `Py_TPFLAGS_BASETYPE` flag, so they may be subclassed; `slice` does not
    /// have it and raises `TypeError: type 'slice' is not an acceptable base
    /// type`.
    const fn non_subclassable_name(self) -> Option<&'static str> {
        match self {
            Self::Slice => Some("slice"),
            _ => None,
        }
    }

    /// This thread's `PyClass` singleton.
    pub(crate) fn singleton(self) -> Rc<RefCell<PyClass>> {
        BUILTIN_TYPE_CLASSES.with(|classes| Rc::clone(&classes[self as usize]))
    }
}

/// Classify a primitive class by its immutable interpreter-owned tag.
///
/// Unlike reading `PyClass::name`, this remains correct if Python-visible
/// metadata is changed internally, and a user class named `list`/`int`/etc.
/// can never be mistaken for the corresponding built-in.
#[inline]
pub(crate) fn primitive_class_kind(class: &Rc<RefCell<PyClass>>) -> Option<PrimitiveClassKind> {
    class
        .borrow()
        .canonical_tag
        .filter(|tag| tag.is_primitive())
}
