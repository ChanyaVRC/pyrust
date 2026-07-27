/// Cached, MRO-resolved construction facts for a single-inheritance class — the
/// values `instantiate_normal_instance` otherwise re-derives by walking the base
/// chain on *every* `Cls(...)` call (issue #2330).  Stored behind a `Box` on the
/// lazily-populated `PyClass::construction_cache`, so an un-constructed class
/// carries only a null pointer (`None`).
///
/// The interpreter owns the *meaning* of these fields; the core only provides
/// storage and the two validity stamps. `primitive_tag` records the immutable
/// identity of the primitive base, if any.
#[derive(Debug, Clone)]
pub struct CachedConstructionPlan {
    /// MRO-resolved `__new__`, or `None` if none found.
    pub new_val: Option<Value>,
    /// MRO-resolved `__init__`, or `None`.
    pub init_val: Option<Value>,
    /// Immutable identity of the primitive base, if one supplies storage.
    pub primitive_tag: Option<CanonicalClassTag>,
    /// `PyClass::mutation_version` captured when this plan was resolved.  A
    /// mismatch means *this* class was mutated since.
    pub class_version: u64,
    /// `class_epoch()` captured when resolved.  A mismatch means *some* class
    /// (possibly a base in the chain) was mutated since — invalidates the cache
    /// conservatively, mirroring how the attribute inline caches re-validate.
    pub epoch: u64,
}

/// Immutable identity tag for interpreter-owned classes whose semantics must
/// survive Python-visible `__name__` / `__qualname__` changes.
///
/// Ordinary user classes never receive a tag.  The interpreter assigns one
/// only while constructing its canonical primitive/object singletons.
/// Cross-crate protocol code can therefore distinguish a real runtime-owned
/// built-in class from a same-named user class without depending on mutable
/// presentation metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalClassTag {
    Object,
    Bool,
    Bytearray,
    Bytes,
    Complex,
    Dict,
    Ellipsis,
    Float,
    Frozenset,
    GenericAlias,
    Int,
    List,
    MappingProxy,
    NoneType,
    NotImplementedType,
    Set,
    Str,
    Tuple,
}

impl CanonicalClassTag {
    /// Map the fixed name used while constructing a primitive singleton to its
    /// internal identity tag.  This is intentionally a construction-time
    /// helper; protocol decisions read the tag and never compare visible names.
    pub fn from_primitive_name(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "bytearray" => Self::Bytearray,
            "bytes" => Self::Bytes,
            "complex" => Self::Complex,
            "dict" => Self::Dict,
            "ellipsis" => Self::Ellipsis,
            "float" => Self::Float,
            "frozenset" => Self::Frozenset,
            "int" => Self::Int,
            "list" => Self::List,
            "mappingproxy" => Self::MappingProxy,
            "NoneType" => Self::NoneType,
            "NotImplementedType" => Self::NotImplementedType,
            "set" => Self::Set,
            "str" => Self::Str,
            "tuple" => Self::Tuple,
            _ => return None,
        })
    }

    pub const fn is_primitive(self) -> bool {
        !matches!(self, Self::Object | Self::GenericAlias)
    }

    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Bool => "bool",
            Self::Bytearray => "bytearray",
            Self::Bytes => "bytes",
            Self::Complex => "complex",
            Self::Dict => "dict",
            Self::Ellipsis => "ellipsis",
            Self::Float => "float",
            Self::Frozenset => "frozenset",
            Self::GenericAlias => "GenericAlias",
            Self::Int => "int",
            Self::List => "list",
            Self::MappingProxy => "mappingproxy",
            Self::NoneType => "NoneType",
            Self::NotImplementedType => "NotImplementedType",
            Self::Set => "set",
            Self::Str => "str",
            Self::Tuple => "tuple",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PyClass {
    pub name: String,
    /// Qualified name (e.g. `Outer.Inner` for nested classes).  Exposed as
    /// `C.__qualname__` via the attribute lookup fast-path in `get_attr`; NOT
    /// stored in `attrs` — CPython keeps `__qualname__` as a type-level
    /// descriptor on `type`, not as an entry in the class's own `__dict__`.
    pub qualname: String,
    /// First (primary) base class, or `None` if there is no explicit base.
    /// Kept as a dedicated `Option` so that the many existing single-inheritance
    /// paths (exception walks, `super()`, primitive-class chains) remain fast
    /// without iterating a `Vec`.
    pub base: Option<Rc<RefCell<PyClass>>>,
    /// Second through Nth bases for multiple inheritance.  Empty for classes
    /// with zero or one explicit base.  Combined with `base`, the full list of
    /// direct bases is `[base] ++ extra_bases` (in declaration order).
    pub extra_bases: Vec<Rc<RefCell<PyClass>>>,
    pub attrs: IndexMap<String, Value>,
    /// Bumped on every `assign_attr` / `delete_attr` on this class (but NOT
    /// its bases — a base mutation is separately detectable via the base's own
    /// counter).  Inline attribute caches store the version at fill time and
    /// re-validate on each hit; a mismatch triggers a slow-path re-fill.
    ///
    /// `u64::MAX` is a saturated sentinel. Class-dependent caches refuse to
    /// fill or hit once it is reached, preventing wraparound ABA after 2^64
    /// mutations. `Cell<u64>` avoids the `borrow_mut()` overhead on the hot
    /// re-validation path.
    pub mutation_version: Cell<u64>,
    pub subclasses: RefCell<Vec<Weak<RefCell<PyClass>>>>,
    /// The metaclass (metatype) of this class.  `None` for classes whose
    /// metatype is the built-in `type` (the common case).  Set to `Some(mcs)`
    /// when the class is created via a custom metaclass (e.g.
    /// `class Foo(metaclass=Meta): pass`).  Used by `type(Foo)` to return the
    /// actual metaclass instead of always returning the `type` singleton
    /// (issue #1626).
    pub metatype: Option<Rc<RefCell<PyClass>>>,
    /// Slot names declared by `__slots__` in this class body (issue #1106).
    /// `None` means no `__slots__` was declared (instances have a full `__dict__`).
    /// `Some(set)` means only attributes whose names are in `set` may be stored
    /// on instances of this class (when no parent class adds a `__dict__` back).
    pub slots: Option<IndexSet<String>>,
    /// Lazily-populated cache of the MRO-resolved construction plan (`__new__` /
    /// `__init__` / primitive-base classification) used by
    /// `instantiate_normal_instance` (issue #2330).  `None` until the class is
    /// first constructed; re-resolved (cheap-path skipped) when `class_version`
    /// or `epoch` no longer match.  `Box`ed so a never-constructed class only
    /// pays a null pointer.  `RefCell` (not `Cell`) because `CachedConstructionPlan`
    /// is not `Copy` (it holds `Option<Value>`); the borrow is short-lived and
    /// never re-entrant.
    pub construction_cache: RefCell<Option<Box<CachedConstructionPlan>>>,
    /// Issue #2335: mirrors CPython's sticky `tp_new == slot_tp_new` state.
    /// Set to `true` the first time `__new__` is assigned to or deleted from
    /// this class at runtime (via `cls.__new__ = ...` / `del cls.__new__`).
    /// CPython installs the generic `slot_tp_new` wrapper on the first such
    /// mutation and never reverts it, so even after the attribute resolves back
    /// to `object.__new__` through the MRO, `object.__new__` still rejects
    /// excess constructor args with "takes exactly one argument".  The flag is
    /// inherited by subclasses through the MRO walk in the excess-args check.
    pub new_slot_wrapped: Cell<bool>,
    /// Verbatim `repr()` override for internal pseudo-class singletons (the
    /// deprecated `typing.List`/`typing.Dict`/… aliases, mirroring CPython's
    /// `_SpecialGenericAlias` which reprs as `typing.List`, not
    /// `<class 'typing.List'>`).  `None` for every ordinary class.  This is a
    /// dedicated field rather than a `__dict__` attribute so a user class cannot
    /// hijack its own repr by defining `__pyrust_class_repr__` (issue #2608);
    /// only the typing module's init code sets it, on its own singletons.
    pub override_repr: Option<Box<str>>,
    /// Stable identity for a canonical interpreter-owned class.
    ///
    /// This tag is internal metadata and cannot be changed through Python
    /// `__name__` / `__qualname__` assignment. Primitive backing and
    /// `NoneType` decisions use it instead of mutable visible names. `None`
    /// denotes an ordinary user-defined class.
    pub canonical_tag: Option<CanonicalClassTag>,
    /// Stable canonical name for a built-in exception class.
    ///
    /// This is internal type metadata, not Python-visible `__name__`.
    /// Exception protocol decisions must use this tag (and the tagged base
    /// chain), because `PyClass::name` is mutable and an unrelated user class
    /// can legitimately have the same visible name as a built-in exception.
    /// `None` for ordinary classes and user-defined exception subclasses.
    pub builtin_exception_name: Option<&'static str>,
    /// Stable display name for built-in classes that cannot be used as a
    /// Python base class.  Owners set this when constructing a final runtime
    /// type; generic class creation consumes the metadata without branching
    /// on a module name or mutable `__name__`.
    pub non_subclassable_name: Option<&'static str>,
}

impl Default for PyClass {
    /// All-default `PyClass`: empty name/qualname, no bases, empty `attrs`,
    /// fresh mutation version, no subclasses, default (`type`) metatype, no
    /// `__slots__`.  Intended for struct-update construction
    /// (`PyClass { name, attrs, ..Default::default() }`) so that adding a new
    /// field only requires a default here, not an edit at every call site.
    fn default() -> Self {
        PyClass {
            name: String::new(),
            qualname: String::new(),
            base: None,
            extra_bases: Vec::new(),
            attrs: IndexMap::new(),
            mutation_version: Cell::new(0),
            subclasses: RefCell::new(Vec::new()),
            metatype: None,
            slots: None,
            construction_cache: RefCell::new(None),
            new_slot_wrapped: Cell::new(false),
            override_repr: None,
            canonical_tag: None,
            builtin_exception_name: None,
            non_subclassable_name: None,
        }
    }
}

impl PyClass {
    /// Advance this class's direct-mutation version without permitting ABA.
    #[inline]
    pub fn bump_mutation_version(&self) {
        self.mutation_version
            .set(self.mutation_version.get().saturating_add(1));
    }

    /// Construct a `PyClass` from the four commonly-varying fields, defaulting
    /// the rest (`extra_bases` empty, `metatype` `None`, `slots` `None`, fresh
    /// `mutation_version`, empty `subclasses`).  Sites that need a non-default
    /// `extra_bases` / `metatype` / `slots` use struct-update syntax on top of
    /// [`PyClass::default`] instead.
    pub fn new(
        name: impl Into<String>,
        qualname: impl Into<String>,
        base: Option<Rc<RefCell<PyClass>>>,
        attrs: IndexMap<String, Value>,
    ) -> Self {
        PyClass {
            name: name.into(),
            qualname: qualname.into(),
            base,
            attrs,
            ..PyClass::default()
        }
    }
}

#[cfg(test)]
mod cache_version_tests {
    use super::PyClass;

    #[test]
    fn class_mutation_version_saturates_instead_of_wrapping() {
        let class = PyClass::default();
        class.mutation_version.set(u64::MAX - 1);
        class.bump_mutation_version();
        assert_eq!(class.mutation_version.get(), u64::MAX);
        class.bump_mutation_version();
        assert_eq!(class.mutation_version.get(), u64::MAX);
        assert_eq!(crate::class_cache_stamp(class.mutation_version.get()), None);
    }
}

thread_local! {
    /// Per-thread interner for instance attribute-name keys.
    ///
    /// Every `PyInstance` stores its attribute names as `Rc<str>`.  Without
    /// interning, N instances of the same class would each heap-allocate the
    /// SAME key strings (`"x"`, `"y"`, …) — the dominant per-instance memory
    /// cost identified in #2012.  This table maps a name to a single shared
    /// `Rc<str>`, so the bytes for each distinct attribute name are allocated
    /// exactly once and refcount-shared across all instances.
    ///
    /// Bounded like the string intern table: only short names (identifiers,
    /// dunders) are interned, and the table is capped so pathological programs
    /// that synthesise unbounded distinct names don't grow it without limit.
    /// Names outside those bounds simply get a fresh (non-shared) `Rc<str>`,
    /// which is still correct — interning is a memory optimisation, not a
    /// semantic requirement.
    static ATTR_KEY_INTERN: RefCell<HashMap<Box<str>, Rc<str>>> =
        RefCell::new(HashMap::new());
}

/// Maximum byte length of an attribute name eligible for key interning.
const ATTR_KEY_INTERN_MAX_BYTES: usize = 40;
/// Maximum number of distinct interned attribute names per thread.
const ATTR_KEY_INTERN_MAX_ENTRIES: usize = 4096;

/// Return a shared `Rc<str>` for attribute name `name`, reusing the cached
/// allocation when one exists.  Falls back to a fresh `Rc<str>` for names that
/// are too long or once the table is full (still correct, just not shared).
pub fn intern_attr_key(name: &str) -> Rc<str> {
    if name.len() > ATTR_KEY_INTERN_MAX_BYTES {
        return Rc::from(name);
    }
    ATTR_KEY_INTERN.with(|cache| {
        let mut map = cache.borrow_mut();
        if let Some(k) = map.get(name) {
            return Rc::clone(k);
        }
        let k: Rc<str> = Rc::from(name);
        if map.len() < ATTR_KEY_INTERN_MAX_ENTRIES {
            map.insert(name.into(), Rc::clone(&k));
        }
        k
    })
}
