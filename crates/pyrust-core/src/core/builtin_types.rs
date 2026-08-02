// ─────────────────────────────────────────────────────────────────────────────
// BuiltinTypeOps — operations the VM performs on built-in objects whose
// concrete implementation lives in `pyrust-builtins`.  `pyrust-core` never
// names a concrete built-in type; the VM dispatches through this trait.
//
// `state` is `Rc<RefCell<Box<dyn Any>>>` so impls can downcast to their
// concrete state and `RefCell::borrow_mut` when they need to mutate.  Default
// methods return CPython-style "object is not X" errors; impls override only
// the operations their type actually supports.
// ─────────────────────────────────────────────────────────────────────────────

pub type BuiltinState = Rc<RefCell<Box<dyn Any>>>;

pub trait BuiltinTypeOps: Any {
    fn type_name(&self) -> &'static str;

    /// Immutable primitive identity for a built-in object that backs a
    /// canonical Python class.
    ///
    /// `type_name()` is presentation metadata. Core policy consumes this tag
    /// instead of interpreting that display string. Most opaque built-ins are
    /// not primitive backings and keep the default `None`.
    fn canonical_class_tag(&self) -> Option<CanonicalClassTag> {
        None
    }

    /// Python-visible class name for diagnostics and type presentation.
    ///
    /// `type_name()` remains the stable implementation/registry name for
    /// opaque built-ins. Canonical primitive backings publish a typed tag, so
    /// generic runtime code can present their Python class without decoding an
    /// implementation-specific string.
    fn display_type_name(&self) -> &'static str {
        self.canonical_class_tag()
            .map_or_else(|| self.type_name(), CanonicalClassTag::canonical_name)
    }

    /// CPython's `tp_name` spelling used only in error messages.
    ///
    /// Most opaque built-ins use their Python-visible display name. Static
    /// stdlib types whose diagnostics are module-qualified override this
    /// without changing repr or type presentation.
    fn display_error_name(&self) -> &'static str {
        self.display_type_name()
    }

    /// Override the backing state address used for Python object identity.
    ///
    /// Most built-in objects are identified by their shared [`BuiltinState`]
    /// cell and keep `None`.  A proxy whose Python identity belongs to another
    /// live target returns that target's exact pointer payload instead.  Core
    /// combines an override with this ops implementation's concrete `TypeId`
    /// in a dedicated numeric namespace, so it cannot collide with the target
    /// object itself or an override supplied by another built-in type.
    fn identity_payload(&self, state: &BuiltinState) -> Option<u64> {
        let _ = state;
        None
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let _ = state;
        format!("<{} object>", self.display_type_name())
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        let _ = state;
        true
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let _ = (state, other);
        false
    }

    fn hash(&self, state: &BuiltinState) -> Option<u64> {
        let _ = state;
        None
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let _ = (state, name);
        None
    }

    fn setattr(&self, state: &BuiltinState, name: &str, value: Value) -> Result<()> {
        let _ = (state, value);
        Err(PyError::named(
            "AttributeError",
            format!(
                "'{}' object has no attribute '{}'",
                self.display_error_name(),
                name
            ),
        ))
    }

    fn call(
        &self,
        state: &BuiltinState,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let _ = (state, args, kwargs);
        Err(PyError::named(
            "TypeError",
            format!("'{}' object is not callable", self.display_error_name()),
        ))
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let _ = (state, args, kwargs);
        Err(PyError::named(
            "AttributeError",
            format!(
                "'{}' object has no attribute '{}'",
                self.display_error_name(),
                name
            ),
        ))
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        let _ = state;
        Err(PyError::named(
            "TypeError",
            format!("'{}' object is not iterable", self.display_error_name()),
        ))
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        let _ = state;
        None
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let _ = (state, key);
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object is not subscriptable",
                self.display_error_name()
            ),
        ))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let _ = (state, key, value);
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object does not support item assignment",
                self.display_error_name()
            ),
        ))
    }

    fn delete_item(&self, state: &BuiltinState, key: &Value) -> Result<()> {
        let _ = (state, key);
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object does not support item deletion",
                self.display_error_name()
            ),
        ))
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let _ = (state, item);
        Err(PyError::named(
            "TypeError",
            format!(
                "argument of type '{}' is not iterable",
                self.display_error_name()
            ),
        ))
    }

    /// Returns true if `name` is a method this type exposes.  Used by
    /// `hasattr(x, name)`.  Default returns `false`; impls with a method
    /// table should override.  (We don't probe `call_method` here because
    /// that would require running it with placeholder args, which has
    /// observable side effects.)
    fn has_method(&self, name: &str) -> bool {
        let _ = name;
        false
    }

    /// Returns true if values expose Python's iterable protocol. This is
    /// capability metadata only: generic advancement must additionally require
    /// [`BuiltinTypeOps::is_iterator`] before calling `iter_next`.
    fn is_iterable(&self) -> bool {
        false
    }

    /// Returns true if this type is *itself an iterator* — i.e. its
    /// `__iter__` returns `self`, per the iterator protocol.  `iter(x)`
    /// returns such objects unchanged instead of wrapping them in a fresh
    /// iterator.  Iterable-but-not-iterator views (e.g. `dict_keys`) must
    /// leave this `false` so `iter()` builds a new iterator over them.
    fn is_iterator(&self) -> bool {
        false
    }

    /// Convert this object to a `PyKey` for use as a dict/set key.  Returns
    /// `None` if this type is not hashable.  Frozensets etc. override this.
    fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
        let _ = state;
        None
    }

    /// Produce an *independent* copy of this object's storage — the built-in
    /// half of `copy.copy`.  The returned value must share no mutable backing
    /// with `state`, so writing to it can never be observed through the
    /// original; the element `Value`s it holds are shared (shallow-copy
    /// semantics).  Any per-object bookkeeping that only describes the
    /// original (mutation counters observed by live iterators, for example)
    /// starts fresh in the copy.
    ///
    /// Immutable and identity-like built-ins keep the default `None`, which
    /// tells the `copy` module to hand back the original object unchanged.
    fn copy_storage(&self, state: &BuiltinState) -> Option<Value> {
        let _ = state;
        None
    }

    /// The `Value` payload this object's storage holds, in iteration order.
    /// Only container storages override it; `copy.deepcopy` uses it to recurse
    /// into the payload of a storage returned by
    /// [`BuiltinTypeOps::copy_storage`].
    fn storage_elements(&self, state: &BuiltinState) -> Option<Vec<Value>> {
        let _ = state;
        None
    }

    /// Re-seat the payload of a storage produced by
    /// [`BuiltinTypeOps::copy_storage`].  Returns `false` when the type does
    /// not support element replacement.
    fn set_storage_elements(&self, state: &BuiltinState, elements: Vec<Value>) -> bool {
        let _ = (state, elements);
        false
    }

    /// Is this an implementation-internal payload parked in a Python object's
    /// attributes rather than a user-visible value of its own?  Copying the
    /// owning object must give the copy its own storage instead of aliasing
    /// this one (issue #2935).
    fn is_internal_storage(&self) -> bool {
        false
    }
}

/// Return whether a type-erased built-in operations table has concrete type
/// `T`.
///
/// Built-in operation tables are commonly zero-sized singletons, so comparing
/// their data pointers is not a sound identity test: Rust may give distinct
/// zero-sized statics the same address. `Any::type_id` identifies the concrete
/// implementation type instead and is independent of Python-visible
/// [`BuiltinTypeOps::type_name`] presentation metadata.
#[inline]
pub fn builtin_ops_is<T: BuiltinTypeOps>(ops: &dyn BuiltinTypeOps) -> bool {
    ops.type_id() == std::any::TypeId::of::<T>()
}

/// Registry function: maps a stable type-name string to its `BuiltinTypeOps`.
/// Installed once at interpreter startup by the consumer of pyrust-core
/// (typically `pyrust-builtins`).  `pyrust-core` never names a concrete
/// built-in type — it only looks up by string and calls through the trait.
pub type BuiltinRegistry = fn(&str) -> Option<&'static dyn BuiltinTypeOps>;

static BUILTIN_REGISTRY: std::sync::OnceLock<BuiltinRegistry> = std::sync::OnceLock::new();

/// Install the registry that maps built-in type names to their dispatch ops.
/// Safe to call multiple times — only the first call wins.
pub fn install_builtin_registry(registry: BuiltinRegistry) {
    let _ = BUILTIN_REGISTRY.set(registry);
}

/// Look up dispatch ops for a built-in type name.  Returns `None` if no
/// registry has been installed or the type is unknown.
pub fn lookup_builtin_ops(type_name: &str) -> Option<&'static dyn BuiltinTypeOps> {
    BUILTIN_REGISTRY.get().and_then(|reg| reg(type_name))
}

/// Callback for materialising an arbitrary `Value` into its iteration items.
/// Installed by `pyrust` (which owns the interpreter's `iter_values` impl)
/// so that `pyrust-builtins` iterator helpers can drain a source value
/// without depending on the interpreter crate.
///
/// The helpers (`enumerate`/`zip`/`reversed`) call this lazily — at first
/// `iter_next` invocation — to preserve side-effect timing: side effects of
/// the source (e.g. `open()` reading a file) happen at iteration start, not
/// at helper construction.
pub type IterValuesFn = fn(&Value) -> Result<Vec<Value>>;

static ITER_VALUES_FN: std::sync::OnceLock<IterValuesFn> = std::sync::OnceLock::new();

pub fn install_iter_values(f: IterValuesFn) {
    let _ = ITER_VALUES_FN.set(f);
}

pub fn iter_values_via_registry(value: &Value) -> Result<Vec<Value>> {
    match ITER_VALUES_FN.get() {
        Some(f) => f(value),
        None => Err(PyError::Runtime(
            "iter_values callback not installed".to_string(),
        )),
    }
}

/// Callback for the canonical Python `<` ordering between two `Value`s.
/// Installed by `pyrust` (which owns the interpreter's `compare_values`
/// in `interpreter/helpers.rs`) so that `pyrust-builtins` sort helpers
/// can route through the same predicate the `<` / `>` operators use —
/// covering BigInt, nested List, and any other types the interpreter
/// supports — without depending on the interpreter crate.
///
/// Mirrors the [`IterValuesFn`] / [`install_iter_values`] pattern (see
/// issue #428 — duplicate `compare_values` in `pyrust-builtins::list`
/// was missing BigInt and List support, so `list.sort()` / `sorted()`
/// raised `TypeError` on values the `<` operator accepted).
pub type CompareValuesFn = fn(&Value, &Value) -> Result<std::cmp::Ordering>;

static COMPARE_VALUES_FN: std::sync::OnceLock<CompareValuesFn> = std::sync::OnceLock::new();

pub fn install_compare_values(f: CompareValuesFn) {
    let _ = COMPARE_VALUES_FN.set(f);
}

pub fn compare_values_via_registry(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
    match COMPARE_VALUES_FN.get() {
        Some(f) => f(a, b),
        None => Err(PyError::Runtime(
            "compare_values callback not installed".to_string(),
        )),
    }
}

/// Type homogeneity of a to-be-sorted slice, used to pick a specialized native
/// comparator (CPython's `unsafe_long_compare` / `unsafe_latin_compare`) over the
/// general comparison dispatch.  Shared by `sorted()` and `list.sort`.
pub enum SortKind {
    /// Every element is a small `int` — compare via `as_int` (no type dispatch,
    /// no `Result`, cannot raise).
    AllInt,
    /// Every element is a `str` — compare via `as_str`.
    AllStr,
    /// At least one `PyInstance` — comparisons may fire user `__lt__`, so the
    /// caller must route through the interpreter comparator.
    HasInstance,
    /// Mixed / other primitives — use the general comparator.
    General,
}

/// One pass over the sort elements (or keys) to classify them for [`SortKind`].
/// A `PyInstance` anywhere forces `HasInstance` (user comparisons); otherwise the
/// result is `AllInt` / `AllStr` only if *every* element is that exact primitive
/// kind.  Takes an iterator so a keyed sort can classify by key with no alloc.
pub fn classify_sort<'a>(items: impl Iterator<Item = &'a Value>) -> SortKind {
    let mut all_int = true;
    let mut all_str = true;
    let mut has_instance = false;
    let mut any = false;
    for v in items {
        any = true;
        match v.kind() {
            ValueKind::Int(_) => all_str = false,
            ValueKind::Str(_) => all_int = false,
            ValueKind::PyInstance(_) => {
                has_instance = true;
                all_int = false;
                all_str = false;
            }
            _ => {
                all_int = false;
                all_str = false;
            }
        }
    }
    if has_instance {
        SortKind::HasInstance
    } else if any && all_int {
        SortKind::AllInt
    } else if any && all_str {
        SortKind::AllStr
    } else {
        SortKind::General
    }
}
