//! Provider-owned metadata for attributes installed on primitive classes.
//!
//! A primitive's existing `METHODS` slice remains the single inventory for
//! methods dispatched on instances.  This module only gives those names an
//! installation category and records the attributes that cannot be expressed
//! by `METHODS`: native class/static descriptors, constructor sentinels, and
//! the small set of slots owned by a derived primitive such as `bool`.

/// How an attribute must be represented in a primitive class dictionary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimitiveClassAttrKind {
    /// Ordinary unbound built-in method descriptor.
    InstanceMethod,
    /// C-style classmethod descriptor that binds the concrete class.
    NativeClassMethod,
    /// Staticmethod descriptor whose wrapped callable has stable identity.
    NativeStaticMethod,
    /// `__init__` sentinel used by primitive subclasses and `super()`.
    Init,
    /// `__new__` sentinel used by primitive subclasses and `super()`.
    New,
    /// PEP 585 `__class_getitem__` sentinel.
    ClassGetItem,
    /// A slot implemented by this owner rather than inherited from its base.
    OwnedSlot,
}

/// How interpreter-owned slot-table attributes are materialized on a class.
///
/// Most primitive classes own the slot attributes declared for their runtime
/// kind.  A derived primitive such as `bool` instead inherits the common
/// integer slots through its real base class and materializes only the
/// overrides listed in [`PrimitiveClassAttrs::owned_slots`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrimitiveSlotAttrPolicy {
    #[default]
    MaterializeDeclared,
    InheritExceptOwned,
}

/// One fully classified primitive-class attribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimitiveClassAttr {
    pub name: &'static str,
    pub kind: PrimitiveClassAttrKind,
}

impl PrimitiveClassAttr {
    const fn new(name: &'static str, kind: PrimitiveClassAttrKind) -> Self {
        Self { name, kind }
    }
}

/// Explicit well-known sentinels owned by a primitive type.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PrimitiveClassFlags {
    pub init: bool,
    pub new: bool,
    pub class_getitem: bool,
}

impl PrimitiveClassFlags {
    pub const NONE: Self = Self {
        init: false,
        new: false,
        class_getitem: false,
    };

    pub const fn with_init(mut self) -> Self {
        self.init = true;
        self
    }

    pub const fn with_new(mut self) -> Self {
        self.new = true;
        self
    }

    pub const fn with_class_getitem(mut self) -> Self {
        self.class_getitem = true;
        self
    }
}

/// Complete provider-owned class surface for one primitive.
///
/// `instance_methods` must point at the owner's canonical `METHODS` slice.
/// Names repeated in `native_class_methods` or `native_static_methods` are
/// typed overrides: consumers must not first install them as ordinary methods.
#[derive(Clone, Copy, Debug)]
pub struct PrimitiveClassAttrs {
    pub type_name: &'static str,
    pub instance_methods: &'static [&'static str],
    pub native_class_methods: &'static [&'static str],
    pub native_static_methods: &'static [&'static str],
    pub flags: PrimitiveClassFlags,
    pub owned_slots: &'static [&'static str],
    pub slot_attr_policy: PrimitiveSlotAttrPolicy,
}

impl PrimitiveClassAttrs {
    pub const fn new(type_name: &'static str, instance_methods: &'static [&'static str]) -> Self {
        Self {
            type_name,
            instance_methods,
            native_class_methods: &[],
            native_static_methods: &[],
            flags: PrimitiveClassFlags::NONE,
            owned_slots: &[],
            slot_attr_policy: PrimitiveSlotAttrPolicy::MaterializeDeclared,
        }
    }

    pub const fn with_native_class_methods(mut self, methods: &'static [&'static str]) -> Self {
        self.native_class_methods = methods;
        self
    }

    pub const fn with_native_static_methods(mut self, methods: &'static [&'static str]) -> Self {
        self.native_static_methods = methods;
        self
    }

    pub const fn with_flags(mut self, flags: PrimitiveClassFlags) -> Self {
        self.flags = flags;
        self
    }

    pub const fn with_owned_slots(mut self, slots: &'static [&'static str]) -> Self {
        self.owned_slots = slots;
        self
    }

    pub const fn with_slot_attr_policy(mut self, policy: PrimitiveSlotAttrPolicy) -> Self {
        self.slot_attr_policy = policy;
        self
    }

    /// Classify an explicit override before the `METHODS` default is applied.
    pub fn explicit_kind(&self, name: &str) -> Option<PrimitiveClassAttrKind> {
        if self.native_class_methods.contains(&name) {
            return Some(PrimitiveClassAttrKind::NativeClassMethod);
        }
        if self.native_static_methods.contains(&name) {
            return Some(PrimitiveClassAttrKind::NativeStaticMethod);
        }
        if self.flags.init && name == "__init__" {
            return Some(PrimitiveClassAttrKind::Init);
        }
        if self.flags.new && name == "__new__" {
            return Some(PrimitiveClassAttrKind::New);
        }
        if self.flags.class_getitem && name == "__class_getitem__" {
            return Some(PrimitiveClassAttrKind::ClassGetItem);
        }
        self.owned_slots
            .contains(&name)
            .then_some(PrimitiveClassAttrKind::OwnedSlot)
    }

    /// Iterate the final class attributes in installation order.
    ///
    /// Explicit descriptors are filtered out of the instance-method phase, so
    /// a class/static method is never temporarily visible as an ordinary
    /// `BuiltinFunction`.
    pub fn iter(&self) -> impl Iterator<Item = PrimitiveClassAttr> + '_ {
        let instance_methods = self
            .instance_methods
            .iter()
            .copied()
            .filter(|name| self.explicit_kind(name).is_none())
            .map(|name| PrimitiveClassAttr::new(name, PrimitiveClassAttrKind::InstanceMethod));
        let native_class_methods =
            self.native_class_methods.iter().copied().map(|name| {
                PrimitiveClassAttr::new(name, PrimitiveClassAttrKind::NativeClassMethod)
            });
        let native_static_methods =
            self.native_static_methods.iter().copied().map(|name| {
                PrimitiveClassAttr::new(name, PrimitiveClassAttrKind::NativeStaticMethod)
            });
        let init = self
            .flags
            .init
            .then_some(PrimitiveClassAttr::new(
                "__init__",
                PrimitiveClassAttrKind::Init,
            ))
            .into_iter();
        let new = self
            .flags
            .new
            .then_some(PrimitiveClassAttr::new(
                "__new__",
                PrimitiveClassAttrKind::New,
            ))
            .into_iter();
        let class_getitem = self
            .flags
            .class_getitem
            .then_some(PrimitiveClassAttr::new(
                "__class_getitem__",
                PrimitiveClassAttrKind::ClassGetItem,
            ))
            .into_iter();
        let owned_slots = self
            .owned_slots
            .iter()
            .copied()
            .map(|name| PrimitiveClassAttr::new(name, PrimitiveClassAttrKind::OwnedSlot));

        instance_methods
            .chain(native_class_methods)
            .chain(native_static_methods)
            .chain(init)
            .chain(new)
            .chain(class_getitem)
            .chain(owned_slots)
    }
}

/// `bool` shares integer method dispatch but owns these class attributes.
pub const BOOL: PrimitiveClassAttrs = PrimitiveClassAttrs::new("bool", &[])
    .with_flags(PrimitiveClassFlags::NONE.with_new())
    .with_owned_slots(&[
        "__and__",
        "__or__",
        "__xor__",
        "__rand__",
        "__ror__",
        "__rxor__",
        "__invert__",
    ])
    .with_slot_attr_policy(PrimitiveSlotAttrPolicy::InheritExceptOwned);

/// Resolve metadata for consumers such as `dir()` that start from a type name.
pub fn lookup(type_name: &str) -> Option<&'static PrimitiveClassAttrs> {
    match type_name {
        crate::int::TYPE_NAME => Some(&crate::int::CLASS_ATTRS),
        crate::float::TYPE_NAME => Some(&crate::float::CLASS_ATTRS),
        crate::complex::TYPE_NAME => Some(&crate::complex::CLASS_ATTRS),
        crate::list::TYPE_NAME => Some(&crate::list::CLASS_ATTRS),
        crate::tuple::TYPE_NAME => Some(&crate::tuple::CLASS_ATTRS),
        crate::dict::TYPE_NAME => Some(&crate::dict::CLASS_ATTRS),
        crate::set::TYPE_NAME => Some(&crate::set::CLASS_ATTRS),
        crate::frozenset::TYPE_NAME => Some(&crate::frozenset::CLASS_ATTRS),
        crate::string::TYPE_NAME => Some(&crate::string::CLASS_ATTRS),
        crate::bytes::TYPE_NAME => Some(&crate::bytes::CLASS_ATTRS),
        crate::bytearray::TYPE_NAME => Some(&crate::bytearray::CLASS_ATTRS),
        "bool" => Some(&BOOL),
        _ => None,
    }
}
