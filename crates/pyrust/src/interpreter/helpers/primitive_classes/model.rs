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
