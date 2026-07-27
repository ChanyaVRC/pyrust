// Primitive subclass storage layout.
//
// Exact built-in type names belong to class metadata. Call construction
// consumes this typed classification and does not maintain its own type-name
// table.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimitiveLayout {
    /// No primitive storage layout appears in the base chain.
    None,
    /// Mutable containers need an empty backing value before `__init__`.
    Mutable(PrimitiveClassKind),
    /// Immutable containers build their backing value from constructor args.
    Immutable(PrimitiveClassKind),
    /// Scalar primitives build their backing value from constructor args.
    Scalar(PrimitiveClassKind),
}

impl PrimitiveLayout {
    #[inline]
    pub(super) fn primitive_kind(self) -> Option<PrimitiveClassKind> {
        match self {
            Self::None => None,
            Self::Mutable(kind) | Self::Immutable(kind) | Self::Scalar(kind) => Some(kind),
        }
    }

    #[inline]
    pub(super) fn from_primitive_kind(kind: Option<PrimitiveClassKind>) -> Self {
        kind.map_or(Self::None, primitive_layout_for_kind)
    }
}

#[inline]
fn primitive_layout_for_kind(kind: PrimitiveClassKind) -> PrimitiveLayout {
    match kind {
        PrimitiveClassKind::Dict | PrimitiveClassKind::List | PrimitiveClassKind::Set => {
            PrimitiveLayout::Mutable(kind)
        }
        PrimitiveClassKind::Frozenset | PrimitiveClassKind::Tuple => {
            PrimitiveLayout::Immutable(kind)
        }
        PrimitiveClassKind::Str
        | PrimitiveClassKind::Int
        | PrimitiveClassKind::Float
        | PrimitiveClassKind::Bytes
        | PrimitiveClassKind::Bytearray
        | PrimitiveClassKind::Complex => PrimitiveLayout::Scalar(kind),
        _ => PrimitiveLayout::None,
    }
}

/// Return the storage layout attached to a primitive class singleton.
///
/// Call construction consumes this typed result and never classifies a class
/// from its Python-visible name.
#[inline]
pub(super) fn primitive_layout_for_class(class: &Rc<RefCell<PyClass>>) -> PrimitiveLayout {
    primitive_class_kind(class).map_or(PrimitiveLayout::None, primitive_layout_for_kind)
}

/// Find the primitive storage layout inherited by `class`, if any.
#[inline]
pub(super) fn classify_primitive_layout(class: &Rc<RefCell<PyClass>>) -> PrimitiveLayout {
    let mut current = Rc::clone(class);
    loop {
        let layout = primitive_layout_for_class(&current);
        if layout != PrimitiveLayout::None {
            return layout;
        }

        let base = current.borrow().base.clone();
        match base {
            Some(base) => current = base,
            None => return PrimitiveLayout::None,
        }
    }
}
