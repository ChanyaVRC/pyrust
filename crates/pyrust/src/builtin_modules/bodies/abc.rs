// `abc` module — native Rust implementation of CPython 3.12's `abc` (and the
// `_abc` accelerator it is built on), issues #2612 / #3293.
//
// The module exports the `ABCMeta` metaclass, the `ABC` helper base class, the
// `@abstractmethod` decorator (plus the deprecated `abstractclassmethod` /
// `abstractstaticmethod` / `abstractproperty`), and the `get_cache_token` /
// `update_abstractmethods` helpers.
//
// Nothing here is Python source: `import abc` — which every default startup
// reaches through `site` → `os` → `os.PathLike` — builds two classes and a
// handful of registry-dispatched callables, with no lexing, parsing or
// bytecode compilation.
//
// Layout, mirroring `Modules/_abc.c`:
//
// - every ABC owns an `_abc_impl` object holding its virtual-subclass
//   registry, positive cache, negative cache and negative-cache version.  The
//   three sets hold classes weakly, so registering a class never keeps it
//   alive;
// - one invalidation counter per GIL group is bumped by every `register()`;
//   a negative cache older than the counter is discarded on its next use;
// - instantiating a class whose own `__abstractmethods__` is non-empty is
//   rejected by the runtime's default `type.__call__` path
//   (`Interpreter::reject_abstract_class`), the same place CPython's
//   `object.__new__` enforces it — `ABCMeta` itself defines no `__call__`.
//
// Reference: <https://docs.python.org/3/library/abc.html>

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, c3_linearize_classes, class_is_subclass_of, render_value_repr,
    type_class_singleton, value_class, value_type_name_str, values_are_identical,
};
use crate::value::{
    InstanceAttrs, PyClass, PyInstance, PyKey, PyModule, PySet, UserFunctionKind, Value, ValueKind,
};

type ClassRc = Rc<RefCell<PyClass>>;

/// Class attribute holding an ABC's [`AbcData`], as in CPython.
const ABC_IMPL_ATTR: &str = "_abc_impl";
const ABSTRACT_METHODS_ATTR: &str = "__abstractmethods__";
const IS_ABSTRACT_ATTR: &str = "__isabstractmethod__";
/// Display name used by the native-state accessors' error messages.
const ABC_DATA_TYPE_NAME: &str = "_abc_data";

/// Registry name of `ABCMeta.__new__`; identifies the defining class when the
/// method chains to the next `__new__` in the metaclass MRO.
const ABC_META_NEW_FN: &str = "abc.ABCMeta.__new__";

const ABC_DOC: &str =
    "Helper class that provides a standard way to create an ABC using\n    inheritance.\n    ";

const ABC_META_DOC: &str = "Metaclass for defining Abstract Base Classes (ABCs).\n\n        \
Use this metaclass to create an ABC.  An ABC can be subclassed\n        \
directly, and then acts as a mix-in class.  You can also register\n        \
unrelated concrete classes (even built-in classes) and unrelated\n        \
ABCs as 'virtual subclasses' -- these and their descendants will\n        \
be considered subclasses of the registering ABC by the built-in\n        \
issubclass() function, but the registering ABC won't show up in\n        \
their MRO (Method Resolution Order) nor will method\n        \
implementations defined by the registering ABC be callable (not\n        \
even via super()).\n        ";

// ── Weak class sets ──────────────────────────────────────────────────────────

/// A set of classes held weakly, keyed by object identity.
///
/// A dead entry keeps its allocation address reserved (the `Weak` pins the
/// control block), so an address can never be reused by a different live
/// class while its entry is still here; dead entries are dropped lazily.
#[derive(Default)]
struct WeakClassSet {
    entries: IndexMap<usize, Weak<RefCell<PyClass>>, rustc_hash::FxBuildHasher>,
}

impl WeakClassSet {
    #[inline]
    fn key(class: &ClassRc) -> usize {
        Rc::as_ptr(class) as usize
    }

    #[inline]
    fn contains(&self, class: &ClassRc) -> bool {
        // The caller holds a strong reference, so a matching key is a live
        // entry for exactly this class.
        self.entries.contains_key(&Self::key(class))
    }

    fn insert(&mut self, class: &ClassRc) {
        // Amortised pruning: sweep dead entries whenever the set has doubled
        // since it was last swept, so a long-lived ABC does not accumulate
        // tombstones for short-lived classes.
        if self.entries.len() >= 32 && self.entries.len().is_power_of_two() {
            self.entries.retain(|_, weak| weak.strong_count() > 0);
        }
        self.entries.insert(Self::key(class), Rc::downgrade(class));
    }

    fn clear(&mut self) {
        self.entries.clear();
    }

    /// The live members, in insertion order.
    fn live(&self) -> Vec<ClassRc> {
        self.entries.values().filter_map(Weak::upgrade).collect()
    }
}

/// Per-ABC state: CPython's `_abc_data`.
///
/// Types the runtime still models as `BuiltinFunction` name tokens
/// (`generator`, `list_iterator`, `builtin_function_or_method`, …) cannot be
/// held weakly; they are immortal, so their verdicts are cached by name.
#[derive(Default)]
struct AbcData {
    registry: WeakClassSet,
    /// Registered types that are still name tokens, in registration order.
    token_registry: indexmap::IndexSet<&'static str, rustc_hash::FxBuildHasher>,
    cache: WeakClassSet,
    negative_cache: WeakClassSet,
    token_cache: rustc_hash::FxHashSet<&'static str>,
    token_negative_cache: rustc_hash::FxHashSet<&'static str>,
    negative_cache_version: u64,
}

/// What `ABCMeta`'s caches can remember a subclass by.
enum CacheKey {
    Class(ClassRc),
    Token(&'static str),
    Uncacheable,
}

impl CacheKey {
    fn of(value: &Value) -> Self {
        match value.kind() {
            ValueKind::PyClass(class) => Self::Class(Rc::clone(class)),
            ValueKind::BuiltinFunction(name) => Self::Token(name),
            _ => Self::Uncacheable,
        }
    }
}

impl AbcData {
    fn cached_positive(&self, key: &CacheKey) -> bool {
        match key {
            CacheKey::Class(class) => self.cache.contains(class),
            CacheKey::Token(name) => self.token_cache.contains(name),
            CacheKey::Uncacheable => false,
        }
    }

    fn cached_negative(&self, key: &CacheKey) -> bool {
        match key {
            CacheKey::Class(class) => self.negative_cache.contains(class),
            CacheKey::Token(name) => self.token_negative_cache.contains(name),
            CacheKey::Uncacheable => false,
        }
    }

    fn remember(&mut self, key: &CacheKey, positive: bool) {
        match (key, positive) {
            (CacheKey::Class(class), true) => self.cache.insert(class),
            (CacheKey::Class(class), false) => self.negative_cache.insert(class),
            (CacheKey::Token(name), true) => {
                self.token_cache.insert(name);
            }
            (CacheKey::Token(name), false) => {
                self.token_negative_cache.insert(name);
            }
            (CacheKey::Uncacheable, _) => {}
        }
    }

    fn clear_negative(&mut self) {
        self.negative_cache.clear();
        self.token_negative_cache.clear();
    }

    fn clear_caches(&mut self) {
        self.cache.clear();
        self.token_cache.clear();
        self.clear_negative();
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: `PyClass` is `!Send + !Sync`, so the `_abc_data`
//     class object cannot live in a process-global `OnceLock`; the
//     invalidation counter is paired with the weak class sets, which are
//     likewise confined to one GIL group.
// (b) Multi-interpreter / multi-threaded impact: interpreters on one GIL group
//     share the opaque `_abc_data` class (it has no Python-visible state) and
//     the counter.  Sharing the counter only ever invalidates a negative
//     cache earlier than strictly necessary; it can never keep a stale entry.
pyrust_core::gil_local! {
    static ABC_DATA_CLASS: ClassRc = {
        let mut attrs: IndexMap<String, Value> = IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("_abc"));
        attrs.insert(
            "__doc__".to_string(),
            Value::string("Internal state held by ABC machinery."),
        );
        Rc::new(RefCell::new(PyClass::new("_abc_data", "_abc_data", None, attrs)))
    };
    static ABC_INVALIDATION_COUNTER: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn invalidation_counter() -> u64 {
    ABC_INVALIDATION_COUNTER.with(Cell::get)
}

/// Build a fresh `_abc_data` object for one ABC.
///
/// Also used by the native modules that wire a hand-built class onto
/// `ABCMeta` (`os.PathLike`, the `importlib` ABCs), so those classes carry the
/// same `_abc_impl` slot a class created through `ABCMeta.__new__` does.
pub(crate) fn new_abc_impl() -> Value {
    let class = ABC_DATA_CLASS.with(Rc::clone);
    let instance = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    native_state::set_native_state(&instance, AbcData::default())
        .expect("a freshly built instance accepts native state");
    instance
}

/// The `_abc_impl` object owned by `class`, created on first use for a class
/// that reached `ABCMeta` without passing through `ABCMeta.__new__`.
///
/// Only the class's *own* dictionary is consulted: an inherited `_abc_impl`
/// belongs to a base ABC, and using it would merge two registries.
fn abc_impl_of(class: &ClassRc) -> Value {
    if let Some(existing) = class.borrow().attrs.get(ABC_IMPL_ATTR)
        && native_state::has_native_state::<AbcData>(existing)
    {
        return existing.clone();
    }
    let created = new_abc_impl();
    let mut class = class.borrow_mut();
    class
        .attrs
        .insert(ABC_IMPL_ATTR.to_string(), created.clone());
    class.bump_mutation_version();
    created
}

fn with_abc_data<R>(class: &ClassRc, f: impl FnOnce(&mut AbcData) -> R) -> Result<R> {
    let data = abc_impl_of(class);
    native_state::with_native_state_mut::<AbcData, R>(&data, ABC_DATA_TYPE_NAME, |data| Ok(f(data)))
}

// ── Small protocol helpers ───────────────────────────────────────────────────

#[inline]
fn positional(value: Value) -> ExpandedCallArg {
    ExpandedCallArg { name: None, value }
}

/// `getattr(target, name, None)`: only `AttributeError` means "absent".
fn optional_attr(interp: &mut Interpreter, target: &Value, name: &str) -> Result<Option<Value>> {
    match interp.get_attr(target, name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.class_name_is("AttributeError") => Ok(None),
        Err(error) => Err(error),
    }
}

/// `bool(getattr(value, "__isabstractmethod__", False))`.
fn has_abstract_flag(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    match optional_attr(interp, value, IS_ABSTRACT_ATTR)? {
        Some(flag) => interp.truthy_value(&flag),
        None => Ok(false),
    }
}

/// Whether `value` is a `classmethod` / `staticmethod` object in either of
/// the runtime's representations (a wrapper object, or a kind-tagged Python
/// function).
fn is_method_wrapper(value: &Value) -> bool {
    if pyrust_builtins::classmethod::as_class_method_any(value).is_some()
        || pyrust_builtins::classmethod::as_static_method_any(value).is_some()
    {
        return true;
    }
    matches!(
        value.kind(),
        ValueKind::UserFunction(function) if function.kind != UserFunctionKind::Regular
    )
}

/// Whether `value` is an abstract method or descriptor.
///
/// CPython asks only `value.__isabstractmethod__`, which `property`,
/// `classmethod` and `staticmethod` compute from the callables they wrap; the
/// wrapped callables are consulted here directly so the answer does not depend
/// on how a descriptor exposes that attribute.
fn is_abstract(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if has_abstract_flag(interp, value)? {
        return Ok(true);
    }
    if let Some((fget, fset, fdel, _)) = pyrust_builtins::property::as_property(value) {
        for accessor in [fget, fset, fdel] {
            if has_abstract_flag(interp, &accessor)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if is_method_wrapper(value)
        && let Some(function) = optional_attr(interp, value, "__func__")?
    {
        return has_abstract_flag(interp, &function);
    }
    Ok(false)
}

/// `isinstance(value, type)`.
///
/// A `BuiltinFunction` name token is what `type(x)` yields for the types the
/// runtime has not migrated to class objects (`type(gen())`, `type(len)`,
/// `type(iter([]))`, …), so it counts as a class here, matching the generic
/// `issubclass` builtin.
fn is_class(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if matches!(
        value.kind(),
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_)
    ) {
        return Ok(true);
    }
    let result = interp.call_function_expanded(
        Value::builtin_function("isinstance"),
        &[
            positional(value.clone()),
            positional(Value::py_class(type_class_singleton())),
        ],
    )?;
    interp.truthy_value(&result)
}

/// `issubclass(class, classinfo)` through the full protocol.
fn is_subclass(interp: &mut Interpreter, class: &Value, classinfo: &Value) -> Result<bool> {
    let result = interp.call_function_expanded(
        Value::builtin_function("issubclass"),
        &[positional(class.clone()), positional(classinfo.clone())],
    )?;
    interp.truthy_value(&result)
}

fn expect_abc(value: &Value, method: &str) -> Result<ClassRc> {
    match value.kind() {
        ValueKind::PyClass(class) => Ok(Rc::clone(class)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "descriptor '{method}' requires a 'abc.ABCMeta' object but received a '{}'",
                value_type_name_str(value)
            ),
        )),
    }
}

// ── Abstract method bookkeeping ──────────────────────────────────────────────

/// Compute and store `cls.__abstractmethods__` (CPython's
/// `compute_abstract_methods`).
fn compute_abstract_methods(interp: &mut Interpreter, class: &ClassRc) -> Result<()> {
    let class_value = Value::py_class(Rc::clone(class));
    let mut abstracts = PySet::default();

    // 1. Names flagged abstract in the class's own namespace.
    let own: Vec<(String, Value)> = class
        .borrow()
        .attrs
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    for (name, value) in &own {
        if is_abstract(interp, value)? {
            abstracts.insert(PyKey::str_from(name));
        }
    }

    // 2. Inherited abstract methods that are still unimplemented.
    let bases: Vec<ClassRc> = {
        let class = class.borrow();
        class
            .base
            .iter()
            .chain(class.extra_bases.iter())
            .cloned()
            .collect()
    };
    for base in bases {
        let base_value = Value::py_class(base);
        let Some(base_abstracts) = optional_attr(interp, &base_value, ABSTRACT_METHODS_ATTR)?
        else {
            continue;
        };
        if base_abstracts.is_none() {
            continue;
        }
        let names = interp.collect_iterable(&base_abstracts).map_err(|error| {
            if error.class_name_is("TypeError") {
                PyError::named("TypeError", "__abstractmethods__ should be iterable")
            } else {
                error
            }
        })?;
        for name in names {
            let Some(text) = name.as_str().map(str::to_string) else {
                // CPython resolves the name with `getattr`, which rejects a
                // non-string name.
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "attribute name must be string, not '{}'",
                        value_type_name_str(&name)
                    ),
                ));
            };
            let Some(value) = optional_attr(interp, &class_value, &text)? else {
                continue;
            };
            if is_abstract(interp, &value)? {
                abstracts.insert(PyKey::str_from(&text));
            }
        }
    }

    interp.assign_attr(
        class_value,
        ABSTRACT_METHODS_ATTR,
        pyrust_builtins::frozenset::frozenset(abstracts),
    )
}

/// Compute `__abstractmethods__` for a `typing.Protocol` subclass.
///
/// CPython builds every protocol class through `_ProtocolMeta`, an `ABCMeta`
/// subclass, so `@abstractmethod` members block instantiation. PyRust's
/// native `Protocol` has no metaclass, so `typing` asks for the same
/// bookkeeping from its `Generic` subclass hook (issue #3326).
pub(crate) fn compute_protocol_abstract_methods(
    interp: &mut Interpreter,
    class: &ClassRc,
) -> Result<()> {
    compute_abstract_methods(interp, class)
}

/// CPython's `_abc._abc_init`: compute the abstract methods of a freshly
/// created ABC and give it its own `_abc_impl`.
fn abc_init(interp: &mut Interpreter, class_value: &Value) -> Result<()> {
    let ValueKind::PyClass(class) = class_value.kind() else {
        // A metaclass further along the MRO returned something that is not a
        // class; there is nothing to initialise.
        return Ok(());
    };
    let class = Rc::clone(class);
    compute_abstract_methods(interp, &class)?;
    interp.assign_attr(class_value.clone(), ABC_IMPL_ATTR, new_abc_impl())
}

// ── ABCMeta protocol ─────────────────────────────────────────────────────────

/// `ABCMeta.__new__(mcls, name, bases, namespace, /, **kwargs)`.
fn abc_meta_new(
    interp: &mut Interpreter,
    mcls: &Value,
    args: &[Value],
    kwargs: IndexMap<String, Value>,
) -> Result<Value> {
    let ValueKind::PyClass(metaclass) = mcls.kind() else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "ABCMeta.__new__(X): X is not a type object ({})",
                value_type_name_str(mcls)
            ),
        ));
    };

    // `super().__new__(mcls, name, bases, namespace, **kwargs)`: continue
    // with the next `__new__` after `ABCMeta` in the metaclass MRO, so a
    // metaclass that combines `ABCMeta` with another metaclass still runs
    // both.  `type.__new__` terminates the chain.
    let defining_class = c3_linearize_classes(metaclass).into_iter().find(|class| {
        matches!(
            class.borrow().attrs.get("__new__").map(Value::kind),
            Some(ValueKind::BuiltinFunction(name)) if name == ABC_META_NEW_FN
        )
    });
    let next_new = match defining_class {
        Some(defining_class) => interp.get_attr(
            &Value::super_proxy_class(defining_class, Rc::clone(metaclass)),
            "__new__",
        )?,
        None => Value::builtin_function("type.__new__"),
    };
    let mut call_args: Vec<ExpandedCallArg> = Vec::with_capacity(1 + args.len() + kwargs.len());
    call_args.push(positional(mcls.clone()));
    call_args.extend(args.iter().cloned().map(positional));
    call_args.extend(kwargs.into_iter().map(|(name, value)| ExpandedCallArg {
        name: Some(name),
        value,
    }));
    let class_value = interp.call_function_expanded(next_new, &call_args)?;
    abc_init(interp, &class_value)?;
    Ok(class_value)
}

/// `ABCMeta.register(cls, subclass)` (CPython's `_abc._abc_register`).
fn abc_register(interp: &mut Interpreter, cls: &Value, subclass: &Value) -> Result<Value> {
    let class = expect_abc(cls, "register")?;
    if !is_class(interp, subclass)? {
        return Err(PyError::named("TypeError", "Can only register classes"));
    }
    if is_subclass(interp, subclass, cls)? {
        return Ok(subclass.clone()); // Already a subclass.
    }
    // Subtle: test for cycles *after* testing for "already a subclass";
    // this means we allow X.register(X) and interpret it as a no-op.
    if is_subclass(interp, cls, subclass)? {
        // This would create a cycle, which is bad for the algorithm below.
        return Err(PyError::named(
            "RuntimeError",
            "Refusing to create an inheritance cycle",
        ));
    }
    match subclass.kind() {
        ValueKind::PyClass(registered) => {
            with_abc_data(&class, |data| data.registry.insert(registered))?;
        }
        ValueKind::BuiltinFunction(name) => {
            with_abc_data(&class, |data| {
                data.token_registry.insert(name);
            })?;
        }
        _ => {}
    }
    // Invalidate negative cache.
    ABC_INVALIDATION_COUNTER.with(|counter| counter.set(counter.get() + 1));
    Ok(subclass.clone())
}

/// `cls.__subclasscheck__(subclass)`, dispatched through the attribute
/// protocol so a metaclass that overrides the hook is honoured.
fn call_subclasscheck(interp: &mut Interpreter, cls: &Value, subclass: Value) -> Result<Value> {
    let hook = interp.get_attr(cls, "__subclasscheck__")?;
    interp.call_function_expanded(hook, &[positional(subclass)])
}

/// `ABCMeta.__instancecheck__(cls, instance)` (CPython's
/// `_abc._abc_instancecheck`).
fn abc_instancecheck(interp: &mut Interpreter, cls: &Value, instance: &Value) -> Result<Value> {
    let class = expect_abc(cls, "__instancecheck__")?;
    let subclass = interp.get_attr(instance, "__class__")?;
    let subtype = value_class(instance);
    let same_type = values_are_identical(&subclass, &subtype);

    // Inline the cache checking.
    let key = CacheKey::of(&subclass);
    if !matches!(key, CacheKey::Uncacheable) {
        let verdict = with_abc_data(&class, |data| {
            if data.cached_positive(&key) {
                return Some(true);
            }
            if same_type
                && data.negative_cache_version == invalidation_counter()
                && data.cached_negative(&key)
            {
                return Some(false);
            }
            None
        })?;
        if let Some(verdict) = verdict {
            return Ok(Value::bool_(verdict));
        }
    }
    if same_type {
        // Fall back to the subclass check.
        return call_subclasscheck(interp, cls, subclass);
    }
    let result = call_subclasscheck(interp, cls, subclass)?;
    if interp.truthy_value(&result)? {
        return Ok(result);
    }
    call_subclasscheck(interp, cls, subtype)
}

/// Whether `class` appears in `subclass.__mro__`.
fn is_direct_subclass(interp: &mut Interpreter, subclass: &Value, class: &ClassRc) -> Result<bool> {
    if let ValueKind::PyClass(subclass) = subclass.kind() {
        return Ok(class_is_subclass_of(subclass, class));
    }
    let Some(mro) = optional_attr(interp, subclass, "__mro__")? else {
        return Ok(false);
    };
    Ok(interp
        .collect_iterable(&mro)?
        .iter()
        .any(|entry| matches!(entry.kind(), ValueKind::PyClass(entry) if Rc::ptr_eq(entry, class))))
}

/// `ABCMeta.__subclasscheck__(cls, subclass)` (CPython's
/// `_abc._abc_subclasscheck`).
fn abc_subclasscheck(interp: &mut Interpreter, cls: &Value, subclass: &Value) -> Result<Value> {
    let class = expect_abc(cls, "__subclasscheck__")?;
    if !is_class(interp, subclass)? {
        return Err(PyError::named(
            "TypeError",
            "issubclass() arg 1 must be a class",
        ));
    }
    // Real class objects are held weakly and name tokens by name; any other
    // class-like value is checked every time without touching the caches.
    let cacheable = CacheKey::of(subclass);
    let remember = |cacheable: &CacheKey, positive: bool| -> Result<()> {
        if !matches!(cacheable, CacheKey::Uncacheable) {
            with_abc_data(&class, |data| data.remember(cacheable, positive))?;
        }
        Ok(())
    };

    // 1. Check cache.  2. Check negative cache; may have to invalidate.
    let counter = invalidation_counter();
    let cached = with_abc_data(&class, |data| {
        if data.cached_positive(&cacheable) {
            return Some(true);
        }
        if data.negative_cache_version < counter {
            // Invalidate the negative cache.
            data.clear_negative();
            data.negative_cache_version = counter;
        } else if data.cached_negative(&cacheable) {
            return Some(false);
        }
        None
    })?;
    if let Some(verdict) = cached {
        return Ok(Value::bool_(verdict));
    }

    // 3. Check the subclass hook.
    let hook = interp.get_attr(cls, "__subclasshook__")?;
    let ok = interp.call_function_expanded(hook, &[positional(subclass.clone())])?;
    match ok.kind() {
        ValueKind::Bool(verdict) => {
            remember(&cacheable, verdict)?;
            return Ok(Value::bool_(verdict));
        }
        ValueKind::NotImplemented => {}
        _ => {
            return Err(PyError::named(
                "AssertionError",
                "__subclasshook__ must return either False, True, or NotImplemented",
            ));
        }
    }

    // 4. Check if it's a direct subclass.
    if is_direct_subclass(interp, subclass, &class)? {
        remember(&cacheable, true)?;
        return Ok(Value::bool_(true));
    }

    // 5. Check if it's a subclass of a registered class (recursive).  The
    //    registry is snapshotted first: the checks below run arbitrary code.
    let registered = with_abc_data(&class, |data| match &cacheable {
        CacheKey::Class(subclass) if data.registry.contains(subclass) => None,
        CacheKey::Token(name) if data.token_registry.contains(name) => None,
        _ => Some((
            data.registry.live(),
            data.token_registry.iter().copied().collect::<Vec<_>>(),
        )),
    })?;
    let Some((registered, registered_tokens)) = registered else {
        // Fast path: the class itself is registered.
        return Ok(Value::bool_(true));
    };
    let entries = registered
        .into_iter()
        .map(Value::py_class)
        .chain(registered_tokens.into_iter().map(Value::builtin_function));
    for entry in entries {
        if is_subclass(interp, subclass, &entry)? {
            remember(&cacheable, true)?;
            return Ok(Value::bool_(true));
        }
    }

    // 6. Check if it's a subclass of a subclass (recursive).
    let subclasses_fn = interp.get_attr(cls, "__subclasses__")?;
    let subclasses = interp.call_function_expanded(subclasses_fn, &[])?;
    let ValueKind::List(subclasses) = subclasses.kind() else {
        return Err(PyError::named(
            "TypeError",
            "__subclasses__() must return a list",
        ));
    };
    let subclasses: Vec<Value> = subclasses.to_vec();
    for scls in subclasses {
        if is_subclass(interp, subclass, &scls)? {
            remember(&cacheable, true)?;
            return Ok(Value::bool_(true));
        }
    }

    // No dice; update negative cache.
    remember(&cacheable, false)?;
    Ok(Value::bool_(false))
}

/// `ABCMeta._dump_registry(cls, file=None)`: debug helper printing the
/// registry and caches.
fn abc_dump_registry(interp: &mut Interpreter, cls: &Value, file: Option<Value>) -> Result<Value> {
    let class = expect_abc(cls, "_dump_registry")?;
    let (registry, cache, negative_cache, version) = with_abc_data(&class, |data| {
        (
            data.registry.live(),
            data.cache.live(),
            data.negative_cache.live(),
            data.negative_cache_version,
        )
    })?;
    let module_name = match optional_attr(interp, cls, "__module__")? {
        Some(module) => match module.as_str() {
            Some(text) => text.to_string(),
            None => render_value_repr(interp, &module)?,
        },
        None => String::new(),
    };
    let qualname = class.borrow().qualname.clone();
    let mut lines = vec![
        format!("Class: {module_name}.{qualname}"),
        format!("Inv. counter: {}", invalidation_counter()),
    ];
    for (label, classes) in [
        ("_abc_registry", registry),
        ("_abc_cache", cache),
        ("_abc_negative_cache", negative_cache),
    ] {
        // CPython prints sets of weak references; the set of classes those
        // references point at carries the same information.
        let rendered = if classes.is_empty() {
            "set()".to_string()
        } else {
            let mut parts = Vec::with_capacity(classes.len());
            for class in classes {
                parts.push(render_value_repr(interp, &Value::py_class(class))?);
            }
            format!("{{{}}}", parts.join(", "))
        };
        lines.push(format!("{label}: {rendered}"));
    }
    lines.push(format!("_abc_negative_cache_version: {version}"));

    let print = Value::builtin_function("print");
    for line in lines {
        let mut call_args = vec![positional(Value::string(line))];
        if let Some(file) = &file {
            call_args.push(ExpandedCallArg {
                name: Some("file".into()),
                value: file.clone(),
            });
        }
        interp.call_function_expanded(print.clone(), &call_args)?;
    }
    Ok(Value::none())
}

/// `abc.update_abstractmethods(cls)`.
fn update_abstract_methods(interp: &mut Interpreter, cls: &Value) -> Result<Value> {
    // A class that never had `__abstractmethods__` is not an ABC (or is a
    // native one whose abstract methods are fixed): leave it alone.
    if optional_attr(interp, cls, ABSTRACT_METHODS_ATTR)?.is_none() {
        return Ok(cls.clone());
    }
    if let ValueKind::PyClass(class) = cls.kind() {
        let class = Rc::clone(class);
        compute_abstract_methods(interp, &class)?;
    }
    Ok(cls.clone())
}

/// Call one of the built-in descriptor constructors on `callable`.
fn wrap_with(
    interp: &mut Interpreter,
    constructor: &'static str,
    callable: Value,
) -> Result<Value> {
    interp.call_function_expanded(
        Value::builtin_function(constructor),
        &[positional(callable)],
    )
}

fn mark_abstract(interp: &mut Interpreter, callable: &Value) -> Result<()> {
    interp.assign_attr(callable.clone(), IS_ABSTRACT_ATTR, Value::bool_(true))
}

// ── Module wiring ────────────────────────────────────────────────────────────

fn module_class(module: &Rc<RefCell<PyModule>>, name: &str) -> Option<ClassRc> {
    match module.borrow().get_attr_value(name)?.kind() {
        ValueKind::PyClass(class) => Some(Rc::clone(class)),
        _ => None,
    }
}

/// Finish the two classes `pyrust_module!` declares: make `ABC` an instance
/// of `ABCMeta` with the empty `__slots__` and ABC state CPython gives it.
pub(crate) fn prepare_module(module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    let (Some(abc_meta), Some(abc)) =
        (module_class(module, "ABCMeta"), module_class(module, "ABC"))
    else {
        return;
    };
    let mut abc = abc.borrow_mut();
    abc.metatype = Some(abc_meta);
    // `__slots__ = ()`: instances of `ABC` itself carry no `__dict__`, and a
    // slotted subclass stays dict-free.
    abc.slots = Some(indexmap::IndexSet::new());
    abc.attrs.insert(
        ABSTRACT_METHODS_ATTR.to_string(),
        pyrust_builtins::frozenset::frozenset(PySet::default()),
    );
    abc.attrs.insert(ABC_IMPL_ATTR.to_string(), new_abc_impl());
}

pyrust_module! {
    constants {
        "__doc__" => Value::string("Abstract Base Classes (ABCs) according to PEP 3119."),
    }

    /// A decorator indicating abstract methods.
    fn abstractmethod(#[positional_only] funcobj: PyValue) -> Result<Value> {
        mark_abstract(_interp, &funcobj.0)?;
        Ok(funcobj.0)
    }

    /// A decorator indicating abstract classmethods (deprecated).
    fn abstractclassmethod(#[positional_only] callable: PyValue) -> Result<Value> {
        mark_abstract(_interp, &callable.0)?;
        wrap_with(_interp, "classmethod", callable.0)
    }

    /// A decorator indicating abstract staticmethods (deprecated).
    fn abstractstaticmethod(#[positional_only] callable: PyValue) -> Result<Value> {
        mark_abstract(_interp, &callable.0)?;
        wrap_with(_interp, "staticmethod", callable.0)
    }

    /// A decorator indicating abstract properties (deprecated).
    fn abstractproperty(#[positional_only] fget: PyValue) -> Result<Value> {
        mark_abstract(_interp, &fget.0)?;
        wrap_with(_interp, "property", fget.0)
    }

    /// Returns the current ABC cache token.
    fn get_cache_token() -> Result<Value> {
        Ok(Value::int(invalidation_counter() as i64))
    }

    /// Recalculate the set of abstract methods of an abstract class.
    fn update_abstractmethods(#[positional_only] cls: PyValueRef<'_>) -> Result<Value> {
        update_abstract_methods(_interp, cls.0)
    }

    class ABCMeta("type") {
        constants {
            "__doc__" => Value::string(ABC_META_DOC),
        }

        fn __new__(
            #[positional_only] mcls: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            abc_meta_new(_interp, mcls.0, args, kwargs)
        }

        /// Register a virtual subclass of an ABC.
        fn register(
            #[positional_only] cls: PyValueRef<'_>,
            subclass: PyValue,
        ) -> Result<Value> {
            abc_register(_interp, cls.0, &subclass.0)
        }

        /// Override for isinstance(instance, cls).
        fn __instancecheck__(
            #[positional_only] cls: PyValueRef<'_>,
            instance: PyValue,
        ) -> Result<Value> {
            abc_instancecheck(_interp, cls.0, &instance.0)
        }

        /// Override for issubclass(subclass, cls).
        fn __subclasscheck__(
            #[positional_only] cls: PyValueRef<'_>,
            subclass: PyValue,
        ) -> Result<Value> {
            abc_subclasscheck(_interp, cls.0, &subclass.0)
        }

        /// Debug helper to print the ABC registry.
        fn _dump_registry(
            #[positional_only] cls: PyValueRef<'_>,
            #[default(None)] file: Option<PyValue>,
        ) -> Result<Value> {
            abc_dump_registry(_interp, cls.0, file.map(|file| file.0).filter(|file| !file.is_none()))
        }

        /// Clear the registry (for debugging or testing).
        fn _abc_registry_clear(#[positional_only] cls: PyValueRef<'_>) -> Result<Value> {
            let class = expect_abc(cls.0, "_abc_registry_clear")?;
            with_abc_data(&class, |data| {
                data.registry.clear();
                data.token_registry.clear();
            })?;
            Ok(Value::none())
        }

        /// Clear the caches (for debugging or testing).
        fn _abc_caches_clear(#[positional_only] cls: PyValueRef<'_>) -> Result<Value> {
            let class = expect_abc(cls.0, "_abc_caches_clear")?;
            with_abc_data(&class, AbcData::clear_caches)?;
            Ok(Value::none())
        }
    }

    class ABC {
        constants {
            "__doc__" => Value::string(ABC_DOC),
            "__slots__" => Value::tuple(Vec::new()),
        }
    }
}
