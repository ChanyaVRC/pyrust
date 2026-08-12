/// Typed identity for a slot stored on an interpreter-owned canonical class.
///
/// Runtime protocol code must not infer this identity from the dispatch key
/// carried by `ValueKind::BuiltinFunction`.  Python code can copy the same
/// descriptor into another class, and presentation names are not ownership.
/// Instead, resolve the immutable canonical owner and compare with its own
/// attribute dictionary through [`value_is_canonical_slot`].
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanonicalSlot {
    ObjectInitSubclass,
    ObjectSubclassHook,
    ObjectGetAttribute,
    ObjectSetAttr,
    ObjectDelAttr,
    ObjectStr,
    ObjectRepr,
    ObjectEq,
    ObjectNe,
    ObjectHash,
    ObjectInit,
    ObjectNew,
    ObjectLt,
    ObjectLe,
    ObjectGt,
    ObjectGe,
    ObjectFormat,
    ObjectSizeof,
    ObjectDir,
    ObjectReduce,
    ObjectReduceEx,
    BaseExceptionReduceEx,
    TypeInit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanonicalSlotOwner {
    Object,
    BaseException,
    Type,
}

impl CanonicalSlot {
    const ALL: [Self; 23] = [
        Self::ObjectInitSubclass,
        Self::ObjectSubclassHook,
        Self::ObjectGetAttribute,
        Self::ObjectSetAttr,
        Self::ObjectDelAttr,
        Self::ObjectStr,
        Self::ObjectRepr,
        Self::ObjectEq,
        Self::ObjectNe,
        Self::ObjectHash,
        Self::ObjectInit,
        Self::ObjectNew,
        Self::ObjectLt,
        Self::ObjectLe,
        Self::ObjectGt,
        Self::ObjectGe,
        Self::ObjectFormat,
        Self::ObjectSizeof,
        Self::ObjectDir,
        Self::ObjectReduce,
        Self::ObjectReduceEx,
        Self::BaseExceptionReduceEx,
        Self::TypeInit,
    ];

    const fn owner(self) -> CanonicalSlotOwner {
        match self {
            Self::ObjectInitSubclass
            | Self::ObjectSubclassHook
            | Self::ObjectGetAttribute
            | Self::ObjectSetAttr
            | Self::ObjectDelAttr
            | Self::ObjectStr
            | Self::ObjectRepr
            | Self::ObjectEq
            | Self::ObjectNe
            | Self::ObjectHash
            | Self::ObjectInit
            | Self::ObjectNew
            | Self::ObjectLt
            | Self::ObjectLe
            | Self::ObjectGt
            | Self::ObjectGe
            | Self::ObjectFormat
            | Self::ObjectSizeof
            | Self::ObjectDir
            | Self::ObjectReduce
            | Self::ObjectReduceEx => CanonicalSlotOwner::Object,
            Self::BaseExceptionReduceEx => CanonicalSlotOwner::BaseException,
            Self::TypeInit => CanonicalSlotOwner::Type,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ObjectInitSubclass => "__init_subclass__",
            Self::ObjectSubclassHook => "__subclasshook__",
            Self::ObjectGetAttribute => "__getattribute__",
            Self::ObjectSetAttr => "__setattr__",
            Self::ObjectDelAttr => "__delattr__",
            Self::ObjectStr => "__str__",
            Self::ObjectRepr => "__repr__",
            Self::ObjectEq => "__eq__",
            Self::ObjectNe => "__ne__",
            Self::ObjectHash => "__hash__",
            Self::ObjectInit => "__init__",
            Self::ObjectNew => "__new__",
            Self::ObjectLt => "__lt__",
            Self::ObjectLe => "__le__",
            Self::ObjectGt => "__gt__",
            Self::ObjectGe => "__ge__",
            Self::ObjectFormat => "__format__",
            Self::ObjectSizeof => "__sizeof__",
            Self::ObjectDir => "__dir__",
            Self::ObjectReduce => "__reduce__",
            Self::ObjectReduceEx => "__reduce_ex__",
            Self::BaseExceptionReduceEx => "__reduce_ex__",
            Self::TypeInit => "__init__",
        }
    }

    /// Map an object-owned attribute name to its typed slot identity.
    ///
    /// Exact Python names belong at this canonical-class boundary.  Generic
    /// runtime domains consume the enum and never inspect a qualified
    /// `BuiltinFunction("object.<name>")` key.
    pub(crate) fn object_named(name: &str) -> Option<Self> {
        Some(match name {
            "__init_subclass__" => Self::ObjectInitSubclass,
            "__subclasshook__" => Self::ObjectSubclassHook,
            "__getattribute__" => Self::ObjectGetAttribute,
            "__setattr__" => Self::ObjectSetAttr,
            "__delattr__" => Self::ObjectDelAttr,
            "__str__" => Self::ObjectStr,
            "__repr__" => Self::ObjectRepr,
            "__eq__" => Self::ObjectEq,
            "__ne__" => Self::ObjectNe,
            "__hash__" => Self::ObjectHash,
            "__init__" => Self::ObjectInit,
            "__new__" => Self::ObjectNew,
            "__lt__" => Self::ObjectLt,
            "__le__" => Self::ObjectLe,
            "__gt__" => Self::ObjectGt,
            "__ge__" => Self::ObjectGe,
            "__format__" => Self::ObjectFormat,
            "__sizeof__" => Self::ObjectSizeof,
            "__dir__" => Self::ObjectDir,
            "__reduce__" => Self::ObjectReduce,
            "__reduce_ex__" => Self::ObjectReduceEx,
            _ => return None,
        })
    }
}

thread_local! {
    /// Canonical values resolved from owner dictionaries once per interpreter
    /// thread.  Canonical classes are immutable, so hot repr/hash/comparison
    /// checks need only a TLS access plus `Value` identity comparison instead
    /// of cloning an owner `Rc` and hashing an attribute name on every call.
    static CANONICAL_OBJECT_SLOT_VALUES: Vec<Value> = CanonicalSlot::ALL
        .iter()
        .take_while(|slot| slot.owner() == CanonicalSlotOwner::Object)
        .map(|slot| {
            let owner = object_class_singleton();
            owner.borrow()
                .attrs
                .get(slot.name())
                .cloned()
                .expect("canonical slot must exist on its owner")
        })
        .collect();

    /// Kept separate so an ordinary object-slot check does not force lazy
    /// construction of the canonical `type` singleton.
    static CANONICAL_TYPE_INIT_VALUE: Value = type_class_singleton()
        .borrow()
        .attrs
        .get(CanonicalSlot::TypeInit.name())
        .cloned()
        .expect("canonical type.__init__ slot must exist");

    /// PyRust supplies an exception-specialised `__reduce_ex__` sentinel on
    /// `BaseException`; CPython reaches the same behavior through
    /// `object.__reduce_ex__`. Keep its identity typed so protocol consumers
    /// can recognise the canonical fallback without decoding a function name.
    static CANONICAL_BASE_EXCEPTION_REDUCE_EX_VALUE: Value =
        lookup_exc_class("BaseException")
            .expect("canonical BaseException class must exist")
            .borrow()
            .attrs
            .get(CanonicalSlot::BaseExceptionReduceEx.name())
            .cloned()
            .expect("canonical BaseException.__reduce_ex__ slot must exist");
}

/// Whether `value` is the callable stored in `slot`'s canonical owner's own
/// dictionary.
///
/// This deliberately does not walk an MRO.  The owner and slot are already
/// typed, and the own-dictionary value is the canonical runtime sentinel.
#[inline]
pub(crate) fn value_is_canonical_slot(value: &Value, slot: CanonicalSlot) -> bool {
    match slot.owner() {
        CanonicalSlotOwner::Object => CANONICAL_OBJECT_SLOT_VALUES
            .with(|values| values_are_identical(&values[slot as usize], value)),
        CanonicalSlotOwner::BaseException => CANONICAL_BASE_EXCEPTION_REDUCE_EX_VALUE
            .with(|canonical| values_are_identical(canonical, value)),
        CanonicalSlotOwner::Type => {
            CANONICAL_TYPE_INIT_VALUE.with(|canonical| values_are_identical(canonical, value))
        }
    }
}

#[cfg(test)]
mod canonical_slot_tests {
    use super::*;

    #[test]
    fn canonical_slots_resolve_from_their_immutable_owner() {
        let object = object_class_singleton();
        let repr = object
            .borrow()
            .attrs
            .get("__repr__")
            .cloned()
            .expect("object.__repr__");
        assert!(value_is_canonical_slot(&repr, CanonicalSlot::ObjectRepr));
        assert!(!value_is_canonical_slot(&repr, CanonicalSlot::ObjectStr));

        let base_exception = lookup_exc_class("BaseException").expect("BaseException");
        let reduce_ex = base_exception
            .borrow()
            .attrs
            .get("__reduce_ex__")
            .cloned()
            .expect("BaseException.__reduce_ex__");
        assert!(value_is_canonical_slot(
            &reduce_ex,
            CanonicalSlot::BaseExceptionReduceEx
        ));
        assert!(!value_is_canonical_slot(
            &reduce_ex,
            CanonicalSlot::ObjectReduceEx
        ));

        let type_class = type_class_singleton();
        let init = type_class
            .borrow()
            .attrs
            .get("__init__")
            .cloned()
            .expect("type.__init__");
        assert!(value_is_canonical_slot(&init, CanonicalSlot::TypeInit));
        assert!(!value_is_canonical_slot(&init, CanonicalSlot::ObjectInit));
    }

    #[test]
    fn generic_domains_do_not_decode_canonical_slot_dispatch_keys() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/interpreter/runtime");
        let sources = [
            "attributes/class_attributes.rs",
            "attributes/attribute_assignment.rs",
            "attributes/attribute_deletion.rs",
            "attributes/attribute_cache_policy.rs",
            "value_protocols/operators.rs",
            "expr/numeric_slots.rs",
            "collection_keys/key_conversion.rs",
            "exceptions/render.rs",
            "formatting/value_repr.rs",
            "formatting/value_repr_support.rs",
        ];
        for relative in sources {
            let path = root.join(relative);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
            for forbidden in [
                "BuiltinFunction(\"object.__",
                "BuiltinFunction(\"type.__",
                "==\"object.__",
                "==\"type.__",
            ] {
                assert!(
                    !compact.contains(forbidden),
                    "{} decodes canonical slot identity via {forbidden:?}; use CanonicalSlot",
                    path.display()
                );
            }
        }
    }
}
