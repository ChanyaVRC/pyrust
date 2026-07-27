//! Attribute read/write and method-binding cache protocol.

use crate::value::Value;

/// Per-call-site inline cache entry for `Insn::GetAttr` and `Insn::CallMethod`.
///
/// The cache is indexed by the instruction's position (`pc`) in `FnCode::insns`.
/// Instance entries check that the object is still a `PyInstance` of the same
/// class; native-classmethod entries instead check the `PyClass` target itself.
/// Both shapes validate the weak class identity, `PyClass::mutation_version`, and the global
/// `pyrust_core::class_epoch()`.  The epoch catches mutations to ancestors in
/// the MRO (e.g. `Base.method = new_fn` when a cached target inherits through
/// `Child(Base)`).
///
/// When all guards hold the cached unbound value is rebound to the current
/// instance instead of repeating the `lookup_class_attr` chain walk.
///
/// The value stored is the **unbound** class-attribute value (i.e. what
/// `lookup_class_attr` returns). `GetAttr` rebinds it on each hit; `CallMethod`
/// passes it to `invoke_class_method` which prepends the receiver. Storing the
/// unbound value is critical for correctness: a `BoundMethod` captures a
/// specific receiver and would be stale when the same call site is executed
/// for a different object. The value itself is weak/cache-safe: a method can
/// point back to the `FnCode` containing this cache, so retaining an ordinary
/// `Value` here would form `function -> code -> cache -> function`.
///
/// `Megamorphic` is set when two or more distinct class pointers are observed at
/// the same call site.  Once megamorphic the slot is never re-filled — the slow
/// path is cheaper than the overhead of re-checking on every execution.
///
/// Each entry retains a `Weak<RefCell<PyClass>>`, rather than only the address
/// returned by `Rc::as_ptr`.  A call-site `FnCode` can outlive the class it
/// previously observed.  Keeping the weak allocation identity alive prevents
/// an allocator-reused address from making a different, newly-created class
/// look like the cached class (the ABA problem), without creating a
/// `FnCode -> class -> function -> FnCode` strong-reference cycle.
#[derive(Clone)]
pub enum AttrCacheEntry {
    /// No observation yet — slot is uninitialised.
    Empty,
    /// The observed resolution cannot be represented without retaining a
    /// potentially cyclic value or changing its Python identity. This site
    /// permanently takes the slow path instead of retrying cache fill on every
    /// execution or mislabelling a monomorphic site as megamorphic.
    Uncacheable,
    /// Monomorphic: one class seen.  Validated by pointer + version + epoch check.
    ClassAttr {
        /// Weak allocation identity of the class observed at cache fill.
        class_ptr: std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
        /// Value of `PyClass::mutation_version` when the cache was filled.
        class_version: u64,
        /// Value of `pyrust_core::class_epoch()` when the cache was filled.
        /// Any mutation to any class — including ancestor classes in the MRO —
        /// advances the global epoch, causing this guard to fail.
        epoch: u64,
        /// The unbound class-attr value from `lookup_class_attr`.
        value: pyrust_core::WeakValueCache,
    },
    /// Monomorphic native-classmethod descriptor read on a `PyClass` target.
    ///
    /// Unlike `ClassAttr`, whose target is a `PyInstance` and whose cached
    /// value is rebound to that instance, this entry records an opaque,
    /// target-specific binding plan produced by the classmethod descriptor
    /// provider.  `GetAttr` asks the provider to materialise a fresh bound
    /// builtin on every hit; fused `CallMethod` consumes the direct-call
    /// payload without materialising an unobservable wrapper.
    ///
    /// The target class pointer, its mutation version, and the global class
    /// epoch guard class identity, direct monkeypatches, and inherited
    /// descriptor/MRO changes respectively.
    NativeClassMethod {
        class_ptr: std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
        class_version: u64,
        epoch: u64,
        plan: pyrust_builtins::classmethod::NativeClassMethodCachePlan,
    },
    /// Monomorphic instance-attribute read cache (mirrors CPython's
    /// `LOAD_ATTR_INSTANCE_VALUE`).  Filled when a `GetAttr` site resolves to
    /// the instance `__dict__` with **no data descriptor** shadowing the name
    /// on the class MRO, no custom `__getattribute__`, and no numeric-tower /
    /// `__slots__` complications.  On a hit (same class pointer + version +
    /// epoch) the VM probes `inst.attrs.get(name)` directly, skipping the full
    /// `lookup_class_attr` MRO walk in `get_attr_instance_raw`.
    ///
    /// Correctness: the cached fact is "no data descriptor named `name` exists
    /// on this class's MRO".  That fact is invalidated by the existing
    /// `mutation_version` (this class mutated) + `class_epoch` (any ancestor
    /// mutated) guards.  The instance's class pointer is also part of the guard,
    /// so `__class__` reassignment (#1957/#2102) → different pointer → miss.
    /// If on a hit the name is *not* in the instance dict, the VM falls through
    /// to the slow path (the name may resolve to a method / non-data descriptor
    /// / `__getattr__`), so a missing attribute is always handled correctly.
    InstanceAttr {
        class_ptr: std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
        class_version: u64,
        epoch: u64,
    },
    /// Monomorphic `__slots__` slot read cache (issue #2207).  Filled when a
    /// `GetAttr` site resolves to a `member_descriptor` data descriptor (the
    /// data descriptor installed for each `__slots__` name, #2084).  The cache
    /// retains the descriptor's immutable `MemberSlotId`; a hit reads the
    /// physically separate slot backing with `get_member_slot(slot_id)`, exactly
    /// the storage selected by `member_descriptor.__get__`, while skipping the full
    /// `lookup_class_attr` + data-descriptor dispatch path that made slotted
    /// reads ~15× slower than plain instance reads.
    ///
    /// Correctness: the cached fact is "name `name` resolves to a slot
    /// member_descriptor on this class's MRO, and there is no custom
    /// `__getattribute__`".  Invalidated by the same `class_version` (this class
    /// mutated) + `epoch` (any ancestor mutated) + `class_ptr` (`__class__`
    /// reassignment) guards as `InstanceAttr`.  Crucially, an **unset** member
    /// slot is NOT served from the cache: the VM
    /// falls through to the slow path, which raises the correct
    /// `AttributeError: '<cls>' object has no attribute '<name>'` (and honours
    /// `__getattr__`), preserving the descriptor path's unset-slot semantics
    /// byte-for-byte.
    SlotAttr {
        class_ptr: std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
        class_version: u64,
        epoch: u64,
        slot_id: pyrust_core::MemberSlotId,
    },
    /// Monomorphic instance-attribute write cache (mirrors CPython's
    /// `STORE_ATTR_INSTANCE_VALUE`).  Filled when a `SetAttr` site resolves to a
    /// plain instance `__dict__` write: no `__setattr__` override, no `__set__`
    /// data descriptor on the MRO, not bare `object()`, not an exception slot,
    /// no `__slots__` restriction, and the name is not `__class__` / `__dict__`.
    /// On a hit the VM inserts straight into `inst.attrs`, skipping the
    /// `lookup_class_attr` MRO walk in `assign_attr_instance`.  Same invalidation
    /// as `InstanceAttr`.
    SetInstanceAttr {
        class_ptr: std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
        class_version: u64,
        epoch: u64,
    },
    /// More than one class seen at this site — disable caching.
    Megamorphic,
}

impl AttrCacheEntry {
    pub(crate) fn class_attr(
        class: &std::rc::Rc<std::cell::RefCell<pyrust_core::PyClass>>,
        class_version: u64,
        epoch: u64,
        value: &Value,
    ) -> Self {
        let Some(value) = pyrust_core::WeakValueCache::new(value) else {
            return Self::Uncacheable;
        };
        Self::ClassAttr {
            class_ptr: std::rc::Rc::downgrade(class),
            class_version,
            epoch,
            value,
        }
    }
}

impl std::fmt::Debug for AttrCacheEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttrCacheEntry::Empty => write!(f, "Empty"),
            AttrCacheEntry::Uncacheable => write!(f, "Uncacheable"),
            AttrCacheEntry::ClassAttr {
                class_ptr,
                class_version,
                epoch,
                ..
            } => {
                write!(
                    f,
                    "ClassAttr({:?}, v{class_version}, e{epoch})",
                    class_ptr.as_ptr()
                )
            }
            AttrCacheEntry::InstanceAttr {
                class_ptr,
                class_version,
                epoch,
            } => {
                write!(
                    f,
                    "InstanceAttr({:?}, v{class_version}, e{epoch})",
                    class_ptr.as_ptr()
                )
            }
            AttrCacheEntry::NativeClassMethod {
                class_ptr,
                class_version,
                epoch,
                ..
            } => {
                write!(
                    f,
                    "NativeClassMethod({:?}, v{class_version}, e{epoch})",
                    class_ptr.as_ptr()
                )
            }
            AttrCacheEntry::SlotAttr {
                class_ptr,
                class_version,
                epoch,
                ..
            } => {
                write!(
                    f,
                    "SlotAttr({:?}, v{class_version}, e{epoch})",
                    class_ptr.as_ptr()
                )
            }
            AttrCacheEntry::SetInstanceAttr {
                class_ptr,
                class_version,
                epoch,
            } => {
                write!(
                    f,
                    "SetInstanceAttr({:?}, v{class_version}, e{epoch})",
                    class_ptr.as_ptr()
                )
            }
            AttrCacheEntry::Megamorphic => write!(f, "Megamorphic"),
        }
    }
}
