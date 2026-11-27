pub(crate) fn class_is_subclass_of(
    class: &Rc<RefCell<PyClass>>,
    expected: &Rc<RefCell<PyClass>>,
) -> bool {
    if Rc::ptr_eq(class, expected) {
        return true;
    }
    // The synthetic `object` class is a universal parent: every PyClass
    // (primitive or user-defined) reports it as the terminal of
    // `__mro__`.  `class_is_subclass_of(_, object)` must agree so
    // `issubclass(int, int.__bases__[0])` and `isinstance(x, object)`
    // hold — see Copilot review on #463.
    if Rc::ptr_eq(expected, &object_class_singleton()) {
        return true;
    }
    let (base, extra_bases) = {
        let borrowed = class.borrow();
        (borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    if base.is_some_and(|base| class_is_subclass_of(&base, expected)) {
        return true;
    }
    extra_bases
        .iter()
        .any(|b| class_is_subclass_of(b, expected))
}

/// Compare an exception class against a canonical built-in exception class.
///
/// Walks the immutable canonical exception tags. Python-visible names are not
/// identities and may legitimately be reused by unrelated user classes.
#[inline]
pub(crate) fn class_is_builtin_exception_subclass(
    class: &Rc<RefCell<PyClass>>,
    built_in_name: &str,
) -> bool {
    pyrust_core::class_chain_contains_builtin_exception(class, built_in_name)
}

/// Runtime-side "is this class an exception?" predicate used by the
/// `raise`/`except` machinery.  Answers with the same rule as the canonical
/// `pyrust_core::class_chain_contains_exception` — the base chain reaches the
/// `BaseException` tag — so the runtime and `Value::repr`/`Value::str` paths
/// cannot drift apart (issue #429: divergence here let `raise
/// GeneratorExit(...)` succeed while `repr(GeneratorExit(...))` fell back
/// to the default `<X object>` formatting).  The answer is read from the
/// class's stamped classification, so raising, matching and constructing do
/// not each re-walk the chain (issue #3426).
pub(crate) fn is_exception_class(class: &Rc<RefCell<PyClass>>) -> bool {
    classify_exception_class(class).exception
}

/// The effective `__iter__` for a `PyInstance`: a USER-defined `__iter__`
/// wins; an inherited builtin slot sentinel (`BuiltinFunction("dict.__iter__")`
/// etc., exposed since #2387) on a BACKED instance returns `None`, so callers
/// iterate the backing primitive directly and attach the container-specific
/// mutation guard.  ONE decision shared by the `for`-loop `GetIter` arm and
/// the `iter()` builtin (issue #2400 — the previous mirrored copies were the
/// #2324 duplicate-decision drift pattern).
pub(crate) fn effective_user_iter(
    class: &Rc<RefCell<PyClass>>,
    inst_rc: &Rc<RefCell<PyInstance>>,
) -> Option<Value> {
    lookup_class_attr(class, "__iter__").filter(|method| {
        instance_builtin_data(inst_rc).is_none()
            || !is_inherited_builtin_iter_sentinel(class, method)
    })
}

/// Issue #2335: does any class in `class`'s MRO carry the sticky
/// `new_slot_wrapped` flag — i.e. has `__new__` ever been assigned to or
/// deleted from this class (or an ancestor) at runtime?  CPython installs the
/// generic `slot_tp_new` wrapper on the first such mutation and never reverts
/// it; the wrapped state is inherited by subclasses.  When set, `object.__new__`
/// rejects excess constructor args ("takes exactly one argument") even though
/// the attribute now resolves back to `object.__new__` through the MRO.
pub(crate) fn class_chain_new_slot_wrapped(class: &Rc<RefCell<PyClass>>) -> bool {
    let borrowed = class.borrow();
    if borrowed.new_slot_wrapped.get() {
        return true;
    }
    if let Some(base) = &borrowed.base
        && class_chain_new_slot_wrapped(base)
    {
        return true;
    }
    borrowed
        .extra_bases
        .iter()
        .any(class_chain_new_slot_wrapped)
}

/// Issue #2299: does `class`'s `__hash__` resolve to the implicit
/// `__hash__ = None` that the unhashable built-in types (`list`/`dict`/`set`/
/// `bytearray`) carry on their *type*?  Returns `true` only when an unhashable
/// builtin is reached in the MRO *before* any class supplies its own
/// `__hash__` in its own dict.
///
/// This distinguishes `class L(list): pass` (inherits the builtin's `None` →
/// unhashable) from `class R(list): __hash__ = object.__hash__` (re-enables
/// hashing, so the builtin's `None` is shadowed and `R` is hashable again),
/// matching CPython.  A linear single-inheritance walk suffices for these
/// builtin subclasses; the `extra_bases` branch keeps the same first-defining
/// -class-wins order via the C3 MRO.
pub(crate) fn class_hash_inherits_builtin_none(class: &Rc<RefCell<PyClass>>) -> bool {
    #[inline]
    fn is_unhashable_primitive(class: &Rc<RefCell<PyClass>>) -> bool {
        matches!(
            primitive_class_kind(class),
            Some(
                PrimitiveClassKind::List
                    | PrimitiveClassKind::Dict
                    | PrimitiveClassKind::Set
                    | PrimitiveClassKind::Bytearray
            )
        )
    }

    let borrowed = class.borrow();
    // A class that defines `__hash__` in its own dict shadows anything further
    // down the MRO — so it is *not* unhashable-by-inheritance, regardless of
    // what value it set (`None`, `object.__hash__`, or a function).
    if borrowed.attrs.contains_key("__hash__") {
        return false;
    }
    // An unhashable builtin carries its `__hash__ = None` implicitly (injected
    // by `env.rs::get_attr_class`, not stored in `attrs`).  Reaching one before
    // any explicit `__hash__` means the resolution lands on that `None`.
    if is_unhashable_primitive(class) {
        return true;
    }
    if !borrowed.extra_bases.is_empty() {
        drop(borrowed);
        for cls in c3_linearize_classes(class) {
            let b = cls.borrow();
            if b.attrs.contains_key("__hash__") {
                return false;
            }
            drop(b);
            if is_unhashable_primitive(&cls) {
                return true;
            }
        }
        return false;
    }
    match &borrowed.base {
        Some(base) => class_hash_inherits_builtin_none(base),
        None => false,
    }
}

/// Set of special-exception classifications a class may inherit, all derived
/// in a single non-cloning MRO walk (issue #1967).  Previously
/// `instantiate_exception` ran ~12 separate cloning base-chain scans per
/// constructed exception; this collects every match in one pass.
///
/// Classifications use the immutable canonical exception tag, never the
/// Python-visible class name. User subclasses inherit a classification when
/// the walk reaches their tagged built-in base; unrelated same-named classes
/// do not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExcClassKinds {
    /// `true` if the chain reaches `BaseException`: the class is an exception
    /// class at all.
    pub(crate) exception: bool,
    pub(crate) stop_iteration: bool,
    pub(crate) syntax_error: bool,
    pub(crate) os_error: bool,
    pub(crate) system_exit: bool,
    pub(crate) unicode_decode_error: bool,
    pub(crate) unicode_encode_error: bool,
    pub(crate) unicode_translate_error: bool,
    pub(crate) name_error: bool,
    pub(crate) import_error: bool,
    pub(crate) attribute_error: bool,
    pub(crate) base_exception_group: bool,
    /// `true` if any class in the MRO defines a user (Python) `__new__`.
    /// Plain built-in exceptions and their attribute-only subclasses leave this
    /// `false`, letting `construct_exception_instance` skip the `__new__` MRO
    /// lookup entirely on the hot `raise ValueError("x")` path.  A built-in
    /// `__new__` can never shadow a user `__new__` (built-in `__new__` only
    /// lives on base classes, which are less derived), so a node-wise "any user
    /// `__new__`" test matches the prior MRO-first `.filter(UserFunction)` check.
    pub(crate) has_user_new: bool,
    /// `true` if any class in the MRO defines a user (Python) `__init__`.
    /// Same rationale as [`has_user_new`](Self::has_user_new): `BaseException`
    /// supplies a built-in `__init__`, so plain built-in exceptions stay `false`.
    pub(crate) has_user_init: bool,
}

impl ExcClassKinds {
    fn merge_builtin_exception_name(&mut self, name: Option<&'static str>) {
        let Some(name) = name else {
            return;
        };
        match name {
            "BaseException" => self.exception = true,
            "StopIteration" => self.stop_iteration = true,
            "SyntaxError" => self.syntax_error = true,
            "OSError" => self.os_error = true,
            "SystemExit" => self.system_exit = true,
            "UnicodeDecodeError" => self.unicode_decode_error = true,
            "UnicodeEncodeError" => self.unicode_encode_error = true,
            "UnicodeTranslateError" => self.unicode_translate_error = true,
            "NameError" => self.name_error = true,
            "ImportError" => self.import_error = true,
            "AttributeError" => self.attribute_error = true,
            "BaseExceptionGroup" => self.base_exception_group = true,
            _ => {}
        }
    }

    /// The flags in bit order: flag `i` is bit `i` of [`Self::to_bits`], and
    /// [`Self::from_bits`] reads each one back by the same index.
    fn flags(&self) -> [bool; 14] {
        [
            self.exception,
            self.stop_iteration,
            self.syntax_error,
            self.os_error,
            self.system_exit,
            self.unicode_decode_error,
            self.unicode_encode_error,
            self.unicode_translate_error,
            self.name_error,
            self.import_error,
            self.attribute_error,
            self.base_exception_group,
            self.has_user_new,
            self.has_user_init,
        ]
    }

    /// Pack the classification for `PyClass::exception_kinds_cache`.
    fn to_bits(self) -> u16 {
        self.flags()
            .iter()
            .enumerate()
            .fold(0, |bits, (index, &flag)| bits | (u16::from(flag) << index))
    }

    /// Inverse of [`Self::to_bits`].
    fn from_bits(bits: u16) -> Self {
        let flag = |index: usize| bits & (1 << index) != 0;
        ExcClassKinds {
            exception: flag(0),
            stop_iteration: flag(1),
            syntax_error: flag(2),
            os_error: flag(3),
            system_exit: flag(4),
            unicode_decode_error: flag(5),
            unicode_encode_error: flag(6),
            unicode_translate_error: flag(7),
            name_error: flag(8),
            import_error: flag(9),
            attribute_error: flag(10),
            base_exception_group: flag(11),
            has_user_new: flag(12),
            has_user_init: flag(13),
        }
    }

    /// `true` when construction needs nothing beyond `BaseException`'s own
    /// state (`args` and `__traceback__`): the class belongs to no built-in
    /// family that initialises extra attributes or validates its arguments.
    /// `__new__` / `__init__` overrides are reported separately.
    pub(crate) fn has_base_layout_only(&self) -> bool {
        !(self.stop_iteration
            || self.syntax_error
            || self.os_error
            || self.system_exit
            || self.unicode_decode_error
            || self.unicode_encode_error
            || self.unicode_translate_error
            || self.name_error
            || self.import_error
            || self.attribute_error
            || self.base_exception_group)
    }
}

/// Classify `class` against every special built-in exception name
/// (issue #1967), reusing the class's stamped classification when it is still
/// current (issue #3426).
///
/// The classification is a function of the immutable exception tags and the
/// `__new__` / `__init__` attributes along the base chain.  Every class
/// attribute write bumps the mutated class's `mutation_version` and the
/// global class epoch, so a matching `(version, epoch)` stamp proves no class
/// in the chain changed since the walk that produced the cached bits — the
/// same contract as the per-class construction cache.
pub(crate) fn classify_exception_class(class: &Rc<RefCell<PyClass>>) -> ExcClassKinds {
    let stamp = {
        let borrowed = class.borrow();
        let version = borrowed.mutation_version.get();
        if let Some((cached_version, cached_epoch, bits)) = borrowed.exception_kinds_cache.get()
            && pyrust_core::class_cache_stamp_matches(version, cached_version, cached_epoch)
        {
            return ExcClassKinds::from_bits(bits);
        }
        pyrust_core::class_cache_stamp(version)
    };
    // The walk runs no Python code, so the stamp read above is still the one
    // the classification was derived under.
    let kinds = classify_exception_class_uncached(class);
    if let Some((version, epoch)) = stamp {
        class
            .borrow()
            .exception_kinds_cache
            .set(Some((version, epoch, kinds.to_bits())));
    }
    kinds
}

/// Classify `class` in a single borrowing walk of its base chain
/// (issue #1967).
fn classify_exception_class_uncached(class: &Rc<RefCell<PyClass>>) -> ExcClassKinds {
    let mut kinds = ExcClassKinds::default();
    fn walk(class: &Rc<RefCell<PyClass>>, kinds: &mut ExcClassKinds) {
        let borrowed = class.borrow();
        kinds.merge_builtin_exception_name(borrowed.builtin_exception_name);
        // Detect user-defined or native-stdlib-defined __new__/__init__ in the
        // same walk so plain built-in exceptions (`builtin_exception_name.is_some()`)
        // keep skipping the dedicated MRO lookups.
        if borrowed.builtin_exception_name.is_none() {
            if !kinds.has_user_new
                && matches!(
                    borrowed.attrs.get("__new__").map(Value::kind),
                    Some(ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_))
                )
            {
                kinds.has_user_new = true;
            }
            if !kinds.has_user_init
                && matches!(
                    borrowed.attrs.get("__init__").map(Value::kind),
                    Some(ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_))
                )
            {
                kinds.has_user_init = true;
            }
        }
        if let Some(base) = &borrowed.base {
            walk(base, kinds);
        }
        for b in &borrowed.extra_bases {
            walk(b, kinds);
        }
    }
    walk(class, &mut kinds);
    kinds
}

/// Return `true` if any class in the MRO of `class` (including `class`
/// itself, excluding the implicit `object` root) has `slots: None`.
///
/// CPython rule: `__slots__` only prevents instance `__dict__` creation when
/// *every* class in the MRO (between the leaf class and `object`) declares
/// `__slots__`.  If any ancestor has no `__slots__`, it contributes a
/// `__dict__` and slot enforcement is bypassed.  This covers both:
/// - `class Child(SlottedParent): pass` — Child has `slots: None`
/// - `class GrandChild(Child): __slots__ = ('x',)` — Child has `slots: None`
pub(crate) fn mro_has_unslotted_ancestor(class: &Rc<RefCell<PyClass>>) -> bool {
    // Built-in primitive and canonical types (`dict`, `list`, `set`, `int`,
    // `str`, …) have `tp_dictoffset == 0` in CPython and do not contribute an
    // instance `__dict__` to slotted subclasses (exceptions have
    // `canonical_tag == None` and `slots == None`, so they still count as
    // unslotted ancestors).
    if is_primitive_class(class) {
        return false;
    }
    let (canonical_tag, slots, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.canonical_tag,
            borrowed.slots.clone(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if canonical_tag.is_some() {
        return false;
    }
    if slots.is_none() {
        return true;
    }
    if let Some(ref b) = base
        && !Rc::ptr_eq(b, &object_class_singleton())
        && mro_has_unslotted_ancestor(b)
    {
        return true;
    }
    extra_bases
        .iter()
        .filter(|b| !Rc::ptr_eq(b, &object_class_singleton()))
        .any(mro_has_unslotted_ancestor)
}

/// Return `true` if `name` is listed in the `__slots__` of `class` or any of
/// its ancestors (excluding the implicit `object` root).
///
/// CPython allocates a slot descriptor for every name in `__slots__` along the
/// MRO, so the set of allowed slot names on an instance is the *union* of all
/// `__slots__` across the chain — not just the leaf class's.  This mirrors the
/// traversal of `mro_has_unslotted_ancestor`.
pub(crate) fn mro_slot_allows(class: &Rc<RefCell<PyClass>>, name: &str) -> bool {
    let (slots, base, extra_bases) = {
        let borrowed = class.borrow();
        (
            borrowed.slots.clone(),
            borrowed.base.clone(),
            borrowed.extra_bases.clone(),
        )
    };
    if let Some(ref slot_set) = slots
        && slot_set.contains(name)
    {
        return true;
    }
    if let Some(ref b) = base
        && !Rc::ptr_eq(b, &object_class_singleton())
        && mro_slot_allows(b, name)
    {
        return true;
    }
    extra_bases
        .iter()
        .filter(|b| !Rc::ptr_eq(b, &object_class_singleton()))
        .any(|b| mro_slot_allows(b, name))
}

/// Return `true` if instances of `class` must NOT expose a `__dict__`
/// (issue #2076).  CPython suppresses the instance `__dict__` when the class
/// declares `__slots__`, none of the slots is `'__dict__'`, and no ancestor in
/// the MRO is unslotted (an unslotted ancestor reintroduces `tp_dictoffset`).
/// Mirrors the condition guarding `__slots__` setattr enforcement.
pub(crate) fn class_suppresses_instance_dict(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().slots.is_some()
        && !mro_slot_allows(class, "__dict__")
        && !mro_has_unslotted_ancestor(class)
}

/// Handle `instance.__dict__ = value` (issues #1942 / #1981).
///
/// CPython's `tp_setattro` routes assignment to the `__dict__` slot through a
/// dedicated setter that *replaces* the instance dict wholesale (rather than
/// storing an attribute literally named `__dict__`).  The value must be a
/// `dict`; anything else raises `TypeError`.
///
/// The assigned dict is stored **by reference** as the instance's live backing
/// store (#1981): `obj.__dict__ is d` holds, later mutations of `d` are visible
/// as attribute reads (and vice-versa), and non-str keys round-trip (they're
/// just never attribute-accessible).
///
/// `other.__dict__` evaluates to an `instance_dict` proxy in pyrust (CPython
/// returns the backing dict itself).  A proxy isn't a first-class dict Value,
/// so we materialise a fresh dict from its visible entries and back the
/// instance with that — live aliasing against the *source* instance is not
/// reproduced for the proxy case (out of scope for #1981's criteria).
pub(crate) fn replace_instance_dict(
    instance: &Rc<RefCell<PyInstance>>,
    value: &Value,
) -> Result<()> {
    // Real dict: store it verbatim so identity + aliasing are preserved.
    if value.is_dict() {
        instance.borrow_mut().attrs.set_dict_ref(value.clone());
        return Ok(());
    }
    // instance_dict proxy: no first-class dict to alias; snapshot its visible
    // entries into a fresh dict and back the instance with that.
    match pyrust_builtins::instance_dict::as_instance_dict_items(value) {
        Some(items) => {
            let mut map = pyrust_core::PyDict::default();
            for (k, v) in items {
                map.insert(k, v);
            }
            instance.borrow_mut().attrs.set_dict_ref(Value::dict(map));
            Ok(())
        }
        None => {
            let type_name = pyrust_core::builtin_type_name(value);
            Err(pyrust_core::type_err!(
                "__dict__ must be set to a dictionary, not a '{type_name}'"
            ))
        }
    }
}
