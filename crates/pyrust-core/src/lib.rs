use std::alloc::{Layout, alloc, dealloc};
use std::any::Any;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

pub use num_bigint::BigInt as PyBigInt;
pub use num_traits::ToPrimitive as PyToPrimitive;

static FN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_fn_id() -> u64 {
    FN_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

// Monotonic counter for list/tuple object identity. Each new allocation gets a
// unique id stored at hdr+24; clones copy the same id so `id(x) == id(y)` when
// y is a copy of x, and `id([1]) != id([2])` because they are separate objects.
thread_local! {
    static OBJ_ID_COUNTER: Cell<u64> = const { Cell::new(1) };
}

fn next_obj_id() -> u64 {
    OBJ_ID_COUNTER.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// PyKey — hashable subset of Value used as dict/set keys (unchanged)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyKey {
    Int(i64),
    Float(u64),
    Str(String),
    Bool(bool),
    None,
    /// Hashable frozenset key.  Stores a sorted-canonical Vec of inner keys
    /// so equality and hashing are content-based (matching CPython).
    FrozenSet(Vec<PyKey>),
}

impl Hash for PyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            PyKey::Int(v) => v.hash(state),
            PyKey::Bool(b) => b.hash(state),
            PyKey::Float(bits) => bits.hash(state),
            PyKey::Str(s) => s.hash(state),
            PyKey::None => {}
            PyKey::FrozenSet(items) => {
                for k in items {
                    k.hash(state);
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared types
// ─────────────────────────────────────────────────────────────────────────────

pub type NameSet = Rc<HashSet<String>>;

#[derive(Debug, Clone)]
pub struct FunctionLocals {
    pub slots: Vec<Option<Value>>,
    pub index: Rc<HashMap<String, usize>>,
    pub def_bound_mask: u64,
}

#[derive(Debug, Clone)]
pub struct Environment {
    pub values: HashMap<String, Value>,
    pub fastlocals: Option<FunctionLocals>,
    pub local_names: NameSet,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub parent: Option<EnvRef>,
}

pub type EnvRef = Rc<RefCell<Environment>>;

impl Environment {
    pub fn new(parent: Option<EnvRef>) -> EnvRef {
        Rc::new(RefCell::new(Self {
            values: HashMap::new(),
            fastlocals: None,
            local_names: Rc::new(HashSet::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            parent,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct UserFunctionParam {
    pub name: String,
    pub default: Option<Value>,
    pub is_args: bool,
    pub is_kwargs: bool,
    pub is_keyword_only: bool,
    pub is_positional_only: bool,
}

/// Discriminator for `UserFunction` semantics.  `@classmethod` and
/// `@staticmethod` decorators produce a UserFunction whose body Rc-shares
/// with the original, distinguished only by this tag — no wrapper variant.
/// `Builtin` is the relocated form of the former `Opaque::BuiltinFunction`
/// variant: a Rust built-in dispatched by name (`len`, `print`, …).  Same
/// representable state as the old variant, but unified into the function
/// value's kind tag so `Opaque` shrinks by one variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserFunctionKind {
    #[default]
    Regular,
    ClassMethod,
    StaticMethod,
    Builtin(&'static str),
}

#[derive(Debug, Clone)]
pub struct UserFunction {
    /// Globally unique identity for fn_cache keying — stable across Rc drops/reallocations.
    pub id: u64,
    pub kind: UserFunctionKind,
    pub name: String,
    pub params: Vec<UserFunctionParam>,
    pub local_names: NameSet,
    pub local_index: Rc<HashMap<String, u32>>,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub env: EnvRef,
    pub is_pure: bool,
    pub precompiled_code: Option<Rc<dyn Any>>,
}

#[derive(Debug, Clone)]
pub struct PyClass {
    pub name: String,
    pub base: Option<Rc<RefCell<PyClass>>>,
    pub attrs: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct PyInstance {
    pub class: Rc<RefCell<PyClass>>,
    pub attrs: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct PyModule {
    pub name: String,
    pub attrs: HashMap<String, Value>,
}

// ─────────────────────────────────────────────────────────────────────────────
// NaN-boxing constants
// ─────────────────────────────────────────────────────────────────────────────

const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_SIGN_BIT: u64 = 1 << 47;
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

const TAG_NONE_BITS: u64 = 0xFFF9_0000_0000_0000;
/// Internal-only sentinel for "uninitialised register slot". Positive NaN
/// bit pattern outside the negative-NaN range used by the tag system; not
/// observable from Python code. See `Value::unset()`.
const UNSET_BITS: u64 = 0x7FF8_0000_0000_BAD0;
/// The `NotImplemented` singleton.  Stored as a reserved NaN-box pattern
/// so identity comparison is a cheap u64-eq, and the value doesn't take an
/// `Opaque` variant.  Pattern is a positive NaN in the same family as
/// `UNSET_BITS` — not classified as a float by `top16()`-based checks
/// because we test the exact bit pattern explicitly. See [`Value::not_implemented`].
const NOT_IMPLEMENTED_BITS: u64 = 0x7FF8_0000_0000_BAD2;
const TAG_BOOL_BITS: u64 = 0xFFFA_0000_0000_0000;
const TAG_INT_BITS: u64 = 0xFFFB_0000_0000_0000;
const TAG_STR_BITS: u64 = 0xFFFC_0000_0000_0000;
const TAG_TUPLE_BITS: u64 = 0xFFFD_0000_0000_0000;
const TAG_LIST_BITS: u64 = 0xFFFE_0000_0000_0000;
const TAG_OPAQUE_BITS: u64 = 0xFFFF_0000_0000_0000;

// top16() tag values used in match arms and comparisons
const TAG_FLOAT_MAX: u16 = 0xFFF8; // all top16 values ≤ this are floats
const TAG_NONE: u16 = 0xFFF9;
const TAG_BOOL: u16 = 0xFFFA;
const TAG_INT: u16 = 0xFFFB;
const TAG_STR: u16 = 0xFFFC;
const TAG_TUPLE: u16 = 0xFFFD;
const TAG_LIST: u16 = 0xFFFE;
const TAG_OPAQUE: u16 = 0xFFFF;

fn format_float(v: f64) -> String {
    if v.is_nan() {
        "nan".to_string()
    } else if v.is_infinite() {
        if v > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        }
    } else if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        v.to_string()
    }
}

/// Returns the top 16 bits of a Value's u64 encoding — the tag.
///
/// **Caveat:** the sentinels [`UNSET_BITS`] and [`NOT_IMPLEMENTED_BITS`] are
/// positive-NaN bit patterns whose `top16` is `0x7FF8` (≤ [`TAG_FLOAT_MAX`]).
/// They will be classified as `Float` by a raw `top16`-based check.  Always
/// route through [`Value::kind`] (which checks the exact bit pattern first),
/// [`Value::is_unset`], or [`Value::is_not_implemented`] when distinguishing
/// these from real floats.
#[inline(always)]
fn top16(bits: u64) -> u16 {
    (bits >> 48) as u16
}

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

pub trait BuiltinTypeOps: 'static {
    fn type_name(&self) -> &'static str;

    fn repr(&self, state: &BuiltinState) -> String {
        let _ = state;
        format!("<{} object>", self.type_name())
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
            format!("'{}' object has no attribute '{}'", self.type_name(), name),
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
            format!("'{}' object is not callable", self.type_name()),
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
            format!("'{}' object has no attribute '{}'", self.type_name(), name),
        ))
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        let _ = state;
        Err(PyError::named(
            "TypeError",
            format!("'{}' object is not iterable", self.type_name()),
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
            format!("'{}' object is not subscriptable", self.type_name()),
        ))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let _ = (state, key, value);
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object does not support item assignment",
                self.type_name()
            ),
        ))
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let _ = (state, item);
        Err(PyError::named(
            "TypeError",
            format!("argument of type '{}' is not iterable", self.type_name()),
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

    /// Returns true if this type is iterable.  Default returns `false`;
    /// impls that override `iter_next` must also override this — the VM
    /// uses `is_iterable()` to choose the dispatch path before ever
    /// calling `iter_next`, so an iterable type that forgets to override
    /// `is_iterable` will be treated as non-iterable.
    fn is_iterable(&self) -> bool {
        false
    }

    /// Convert this object to a `PyKey` for use as a dict/set key.  Returns
    /// `None` if this type is not hashable.  Frozensets etc. override this.
    fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
        let _ = state;
        None
    }
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

// ─────────────────────────────────────────────────────────────────────────────
// Shared backing storage for mutable Tier 1 containers
//
// Lists and sets share their items behind an `Rc<…RefCell…>` so that
// `Value::clone` preserves Python's reference semantics for mutable
// containers: a copy of the Value points at the same backing storage as the
// original, mutations through either alias propagate, and `id(a) == id(b)`
// after `b = a`.  Dict already used this shape (`Rc<RefCell<IndexMap<…>>>`
// inside `Opaque::Dict`); see issue #305.
// ─────────────────────────────────────────────────────────────────────────────

/// Shared backing for a Python `list`.  `items` holds the elements; `obj_id`
/// is a monotonic identity captured at construction and inherited by every
/// `Rc::clone` so `id(x) == id(y)` whenever `y` is an aliased clone of `x`.
pub struct ListInner {
    pub items: RefCell<Vec<Value>>,
    pub obj_id: u64,
}

/// Shared backing for a Python `set`.  Same shape and rationale as
/// [`ListInner`]; `items` is an [`IndexSet`] (insertion-ordered) so iteration
/// order matches the rest of the interpreter's set surface.
pub struct SetInner {
    pub items: RefCell<IndexSet<PyKey>>,
    pub obj_id: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Opaque — heap-allocated types that don't fit in 48 bits
// ─────────────────────────────────────────────────────────────────────────────

pub enum Opaque {
    PyBigInt(Rc<BigInt>),
    Dict(Rc<RefCell<IndexMap<PyKey, Value>>>),
    /// Mutable `set` storage.  Shared via `Rc` so `Value::clone` produces an
    /// alias rather than a deep copy, matching Python's reference semantics.
    /// See [`SetInner`] and issue #305.
    Set(Rc<SetInner>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    UserFunction(Rc<UserFunction>),
    PyClass(Rc<RefCell<PyClass>>),
    PyInstance(Rc<RefCell<PyInstance>>),
    PyModule(Rc<RefCell<PyModule>>),
    BoundMethod {
        function: Rc<UserFunction>,
        receiver: Rc<RefCell<PyInstance>>,
    },
    /// A classmethod bound to a specific class (the first argument will be `cls`).
    ClassBoundMethod {
        function: Rc<UserFunction>,
        class: Rc<RefCell<PyClass>>,
    },
    /// Proxy returned by `super(cls, instance)`. Attribute lookup on this proxy
    /// starts from `cls`'s parent class and binds to `instance`.
    ///
    /// Note: zero-argument `super()` (CPython's implicit `__class__` cell) is not
    /// supported. Use the two-argument form `super(CurrentClass, self)` explicitly.
    SuperProxy {
        class: Rc<RefCell<PyClass>>,
        instance: Rc<RefCell<PyInstance>>,
    },
    /// Proxy returned by `super(cls, cls_instance)` where the second argument is
    /// a class (used in classmethods). Attribute lookup starts from `cls`'s parent
    /// and binds as a `ClassBoundMethod` to `obj_class`.
    SuperProxyClass {
        class: Rc<RefCell<PyClass>>,
        obj_class: Rc<RefCell<PyClass>>,
    },
    /// A live generator object.  The concrete execution state (registers, pc,
    /// iterator slots, etc.) is stored as a type-erased `Box<dyn Any>` so that
    /// `pyrust-core` does not need to depend on `pyrust`'s bytecode types.
    Generator(Rc<RefCell<Box<dyn std::any::Any>>>),
    /// An immutable byte string.  Constructed via the `b"..."` literal or
    /// the `bytes(...)` builtin.  Stored behind `Rc` for cheap clones.
    Bytes(Rc<Vec<u8>>),
    /// A Python `complex` number stored as (real, imag).
    Complex(f64, f64),
    /// Inline storage for a 2-element tuple.  Eliminates the secondary
    /// `Vec<Value>` heap allocation for the most common Python tuple shape
    /// (e.g. `dict.items()` entries, `enumerate()` yields, `divmod()`).
    /// `obj_id` preserves stable `id()` identity across clones, matching the
    /// behaviour of the pool-based tuple path.
    SmallTuple2 {
        items: [Value; 2],
        obj_id: u64,
    },
    /// Inline storage for a 3-element tuple.  Same rationale as
    /// `SmallTuple2`; covers `str.partition()`/`rpartition()` and similar
    /// fixed-arity returns.
    SmallTuple3 {
        items: [Value; 3],
        obj_id: u64,
    },
    /// A type-erased built-in object whose operations are dispatched through
    /// the installed [`BuiltinTypeOps`] table.  Used for built-in types whose
    /// payload doesn't justify a dedicated Tier 1 variant (file, property,
    /// frozenset value, dict views, iterator helpers, …).  `pyrust-core`
    /// never names the concrete type — it only calls through `ops`.
    BuiltinObject {
        ops: &'static dyn BuiltinTypeOps,
        state: BuiltinState,
    },
}

impl Clone for Opaque {
    fn clone(&self) -> Self {
        match self {
            Opaque::PyBigInt(rc) => Opaque::PyBigInt(Rc::clone(rc)),
            Opaque::Dict(rc) => Opaque::Dict(Rc::clone(rc)),
            // Sets share backing storage on clone; see `SetInner` and #305.
            Opaque::Set(rc) => Opaque::Set(Rc::clone(rc)),
            Opaque::Range { start, stop, step } => Opaque::Range {
                start: *start,
                stop: *stop,
                step: *step,
            },
            Opaque::UserFunction(f) => Opaque::UserFunction(Rc::clone(f)),
            Opaque::PyClass(c) => Opaque::PyClass(Rc::clone(c)),
            Opaque::PyInstance(i) => Opaque::PyInstance(Rc::clone(i)),
            Opaque::PyModule(m) => Opaque::PyModule(Rc::clone(m)),
            Opaque::BoundMethod { function, receiver } => Opaque::BoundMethod {
                function: Rc::clone(function),
                receiver: Rc::clone(receiver),
            },
            Opaque::ClassBoundMethod { function, class } => Opaque::ClassBoundMethod {
                function: Rc::clone(function),
                class: Rc::clone(class),
            },
            Opaque::SuperProxy { class, instance } => Opaque::SuperProxy {
                class: Rc::clone(class),
                instance: Rc::clone(instance),
            },
            Opaque::SuperProxyClass { class, obj_class } => Opaque::SuperProxyClass {
                class: Rc::clone(class),
                obj_class: Rc::clone(obj_class),
            },
            Opaque::Generator(state) => Opaque::Generator(Rc::clone(state)),
            Opaque::Bytes(rc) => Opaque::Bytes(Rc::clone(rc)),
            Opaque::Complex(re, im) => Opaque::Complex(*re, *im),
            Opaque::SmallTuple2 { items, obj_id } => Opaque::SmallTuple2 {
                items: [items[0].clone(), items[1].clone()],
                obj_id: *obj_id,
            },
            Opaque::SmallTuple3 { items, obj_id } => Opaque::SmallTuple3 {
                items: [items[0].clone(), items[1].clone(), items[2].clone()],
                obj_id: *obj_id,
            },
            Opaque::BuiltinObject { ops, state } => Opaque::BuiltinObject {
                ops: *ops,
                state: Rc::clone(state),
            },
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ValueKind — borrow-based view used for pattern matching
// ─────────────────────────────────────────────────────────────────────────────

pub enum ValueKind<'a> {
    None,
    Bool(bool),
    Int(i64),
    BigInt(&'a BigInt),
    Float(f64),
    Str(&'a str),
    List(&'a [Value]),
    /// Borrowed view of a tuple's elements.  Backed either by the pool path
    /// (`TAG_TUPLE`) which stores `Vec<Value>`, or by the inline
    /// `Opaque::SmallTuple2/3` path which stores a fixed-size array.  Using
    /// a slice unifies both backing representations behind one variant.
    Tuple(&'a [Value]),
    Dict(&'a IndexMap<PyKey, Value>),
    Set(&'a IndexSet<PyKey>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    UserFunction(&'a Rc<UserFunction>),
    BuiltinFunction(&'static str),
    PyClass(&'a Rc<RefCell<PyClass>>),
    PyInstance(&'a Rc<RefCell<PyInstance>>),
    PyModule(&'a Rc<RefCell<PyModule>>),
    BoundMethod {
        function: &'a Rc<UserFunction>,
        receiver: &'a Rc<RefCell<PyInstance>>,
    },
    ClassBoundMethod {
        function: &'a Rc<UserFunction>,
        class: &'a Rc<RefCell<PyClass>>,
    },
    SuperProxy {
        class: &'a Rc<RefCell<PyClass>>,
        instance: &'a Rc<RefCell<PyInstance>>,
    },
    SuperProxyClass {
        class: &'a Rc<RefCell<PyClass>>,
        obj_class: &'a Rc<RefCell<PyClass>>,
    },
    Generator(&'a Rc<RefCell<Box<dyn std::any::Any>>>),
    /// Synthesized view of the [`NotImplemented`] sentinel.  No backing
    /// `Opaque` variant — the value is encoded as a reserved NaN-box bit
    /// pattern, and `kind()` decodes it here so existing matchers keep
    /// working.  Identity-test via `Value::is_not_implemented()` is cheaper.
    NotImplemented,
    Bytes(&'a Rc<Vec<u8>>),
    Complex(f64, f64),
    BuiltinObject {
        ops: &'static dyn BuiltinTypeOps,
        state: &'a BuiltinState,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Thread-local free lists for fixed-size allocations
// ─────────────────────────────────────────────────────────────────────────────

// Each free slot stores a *mut u8 to the next free slot in its first 8 bytes.
thread_local! {
    // (head, len)
    static POOL_B: Cell<(*mut u8, usize)> = const { Cell::new((std::ptr::null_mut(), 0)) };
}

const POOL_B_CAP: usize = 64;

#[inline(always)]
unsafe fn pool_b_alloc() -> *mut u8 {
    POOL_B.with(|c| {
        let (head, len) = c.get();
        if len > 0 {
            let next = unsafe { *(head as *const *mut u8) };
            c.set((next, len - 1));
            head
        } else {
            unsafe { alloc(Layout::from_size_align(20, 8).unwrap()) }
        }
    })
}

#[inline(always)]
unsafe fn pool_b_dealloc(ptr: *mut u8) {
    POOL_B.with(|c| {
        let (head, len) = c.get();
        // In debug builds catch double-free: a block already in the pool has its first
        // 8 bytes overwritten with the next-pointer, so it cannot equal any live
        // allocation's first word.  Check ptr != head as a lightweight guard; a full
        // traversal is too expensive for a hot path.
        debug_assert!(
            ptr != head || head.is_null(),
            "pool_b_dealloc: double-free detected (ptr == head)"
        );
        if len < POOL_B_CAP {
            unsafe { *(ptr as *mut *mut u8) = head };
            c.set((ptr, len + 1));
        } else {
            unsafe { dealloc(ptr, Layout::from_size_align(20, 8).unwrap()) };
        }
    })
}

// Pool for `Box<Opaque>` allocations (#281).  Every `Value::opaque(...)` boxes
// an `Opaque` enum; for hot patterns like `Opaque::SmallTuple2` per-iteration
// these allocations dominate.  Recycling the fixed-size slabs (the enum
// reserves max-variant bytes regardless of which arm is alive) eliminates the
// general allocator round-trip.  Allocator round-trips for non-hot variants
// also benefit; the pool is per-thread and has bounded capacity so it can
// never leak memory unboundedly.
const POOL_OPAQUE_CAP: usize = 128;

thread_local! {
    static POOL_OPAQUE: Cell<(*mut u8, usize)> = const { Cell::new((std::ptr::null_mut(), 0)) };
}

#[inline(always)]
fn opaque_layout() -> Layout {
    Layout::new::<Opaque>()
}

#[inline(always)]
unsafe fn pool_opaque_alloc() -> *mut u8 {
    POOL_OPAQUE.with(|c| {
        let (head, len) = c.get();
        if len > 0 {
            let next = unsafe { *(head as *const *mut u8) };
            c.set((next, len - 1));
            head
        } else {
            unsafe { alloc(opaque_layout()) }
        }
    })
}

#[inline(always)]
unsafe fn pool_opaque_dealloc(ptr: *mut u8) {
    POOL_OPAQUE.with(|c| {
        let (head, len) = c.get();
        if len < POOL_OPAQUE_CAP {
            unsafe { *(ptr as *mut *mut u8) = head };
            c.set((ptr, len + 1));
        } else {
            unsafe { dealloc(ptr, opaque_layout()) };
        }
    })
}

// Pool for Vec<Value> struct headers (list / tuple).
// Layout: [ptr: *mut Value][len: usize][cap: usize][obj_id: u64] = 32 bytes / align 8.
// The extra 8 bytes at offset 24 hold a unique monotonic id for id() identity.

const VEC_HDR_SIZE: usize = 32; // Vec<Value>(24) + obj_id(8) — asserted in Value impl
const VEC_HDR_ALIGN: usize = 8;
const POOL_VEC_HDR_CAP: usize = 64;

thread_local! {
    static POOL_VEC_HDR: Cell<(*mut u8, usize)> = const { Cell::new((std::ptr::null_mut(), 0)) };
}

#[inline(always)]
unsafe fn pool_vec_hdr_alloc() -> *mut u8 {
    POOL_VEC_HDR.with(|c| {
        let (head, len) = c.get();
        if len > 0 {
            let next = unsafe { *(head as *const *mut u8) };
            c.set((next, len - 1));
            head
        } else {
            unsafe { alloc(Layout::from_size_align(VEC_HDR_SIZE, VEC_HDR_ALIGN).unwrap()) }
        }
    })
}

#[inline(always)]
unsafe fn pool_vec_hdr_dealloc(ptr: *mut u8) {
    POOL_VEC_HDR.with(|c| {
        let (head, len) = c.get();
        if len < POOL_VEC_HDR_CAP {
            unsafe { *(ptr as *mut *mut u8) = head };
            c.set((ptr, len + 1));
        } else {
            unsafe {
                dealloc(
                    ptr,
                    Layout::from_size_align(VEC_HDR_SIZE, VEC_HDR_ALIGN).unwrap(),
                )
            };
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Value — NaN-boxed u64
// ─────────────────────────────────────────────────────────────────────────────

#[repr(transparent)]
pub struct Value(u64);

impl Value {
    const _ASSERT_VEC_HDR: () = {
        assert!(std::mem::size_of::<Vec<Value>>() == 24); // Vec<Value> must be 24 bytes
        assert!(std::mem::align_of::<Vec<Value>>() == VEC_HDR_ALIGN);
        assert!(VEC_HDR_SIZE >= 32); // room for obj_id at offset 24
    };

    // ── Constructors ─────────────────────────────────────────────────────────

    pub fn none() -> Self {
        Value(TAG_NONE_BITS)
    }

    /// A distinct, internal-only sentinel representing "register slot not
    /// initialised". The bit pattern is a specific positive NaN that the VM
    /// never produces from real Python values — `0x7FF8_0000_0000_BAD0`.
    ///
    /// `is_unset()` returns true only for this exact pattern. All other
    /// accessors (`kind()`, `truthy()`, `is_none()`, …) classify it as if it
    /// were the corresponding float NaN, which is fine because correct
    /// programs never read an unset slot (the compiler emits `CheckLocal`
    /// before any read that could observe one).
    pub fn unset() -> Self {
        Value(UNSET_BITS)
    }

    pub fn is_unset(&self) -> bool {
        self.0 == UNSET_BITS
    }

    /// `Some(self)` if this slot has been written, else `None`.
    /// Useful for migrating call sites that previously held `Option<Value>`.
    #[inline]
    pub fn as_some(&self) -> Option<&Value> {
        if self.is_unset() { None } else { Some(self) }
    }

    #[inline]
    pub fn as_some_mut(&mut self) -> Option<&mut Value> {
        if self.is_unset() { None } else { Some(self) }
    }

    pub fn bool_(b: bool) -> Self {
        Value(TAG_BOOL_BITS | b as u64)
    }

    pub fn int(n: i64) -> Self {
        const MAX_I48: i64 = (1 << 47) - 1;
        const MIN_I48: i64 = -(1 << 47);
        if (MIN_I48..=MAX_I48).contains(&n) {
            Value(TAG_INT_BITS | (n as u64 & PAYLOAD_MASK))
        } else {
            Value::opaque(Opaque::PyBigInt(Rc::new(BigInt::from(n))))
        }
    }

    pub fn bigint(n: BigInt) -> Self {
        Value::opaque(Opaque::PyBigInt(Rc::new(n)))
    }

    pub fn float(f: f64) -> Self {
        if f.is_nan() {
            Value(CANONICAL_NAN)
        } else {
            Value(f.to_bits())
        }
    }

    pub fn string(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let len = s.len();
        // Layout A: [rc_type:u32][sub_len:u32][ref:*mut u8][bytes: u8 × len]
        //            offset 0     offset 4     offset 8     offset 16
        let layout = Layout::from_size_align(16 + len, 8).unwrap();
        let ptr = unsafe { alloc(layout) };
        unsafe {
            (ptr as *mut u32).write(2u32); // rc=1, type=0
            (ptr.add(4) as *mut u32).write(len as u32);
            // Store the self-referential pointer as *const u8 (immutable bytes).
            (ptr.add(8) as *mut *const u8).write(ptr.add(16)); // ref → own bytes
            if len > 0 {
                ptr.add(16)
                    .copy_from_nonoverlapping(s.as_bytes().as_ptr(), len);
            }
        }
        Value(TAG_STR_BITS | (ptr as u64 & PAYLOAD_MASK))
    }

    pub fn string_slice(&self, byte_start: usize, byte_end: usize) -> Self {
        // Guard against inverted indices: wrapping subtraction would produce a
        // colossal sub_len and the resulting slice descriptor would be invalid.
        assert!(
            byte_start <= byte_end,
            "string_slice: byte_start ({byte_start}) > byte_end ({byte_end})"
        );
        let sub_len = byte_end - byte_start;
        let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
        let rc_type = unsafe { *(hdr as *const u32) };
        // self.ref (offset 8) points to self's bytes[0]; add byte_start for new slice
        let self_ref = unsafe { *(hdr.add(8) as *const *const u8) };
        let new_ref = unsafe { self_ref.add(byte_start) };

        // Find A_ptr (Layout A root) to increment its rc, and compute new offset.
        // Layout A: A_ptr = hdr,   new_offset = byte_start
        // Layout B: A_ptr = ref - stored_offset - 16,  new_offset = stored_offset + byte_start
        //
        // For Layout B→B chains the stored_offset already encodes the distance from A's
        // bytes[0] to this slice's bytes[0], so subtracting it (plus the 16-byte header)
        // from self_ref always recovers A_ptr without underflow.
        let (a_ptr, new_offset): (*mut u8, usize) = if rc_type & 1 == 0 {
            (hdr as *mut u8, byte_start)
        } else {
            let base = unsafe { *(hdr.add(16) as *const u32) as usize };
            // SAFETY: `base` is the byte distance from Layout A's bytes[0] to this
            // slice's bytes[0], written by a prior `string_slice` call.  Therefore
            // `self_ref == a_ptr + 16 + base` by construction, and the subtraction
            // `self_ref - (base + 16)` cannot underflow.  The `byte_start <= byte_end`
            // assert at entry guarantees we never produce an invalid descriptor, so
            // this invariant is preserved through any chain of slices.
            let a_ptr = unsafe { (self_ref as *mut u8).sub(base.wrapping_add(16)) };
            debug_assert!(
                a_ptr as usize + 16 + base == self_ref as usize,
                "string_slice: Layout B offset mismatch — possible heap corruption"
            );
            (a_ptr, base + byte_start)
        };

        // Increment A.rc. Saturate instead of wrapping: a saturated rc leaks the
        // backing buffer, but u32::MAX/2 simultaneous slice references is unreachable.
        unsafe {
            let hdr_a = a_ptr as *mut u32;
            *hdr_a = (*hdr_a).saturating_add(2);
        }

        // Layout B: [rc_type:u32][sub_len:u32][ref:*mut u8][offset:u32]
        //            offset 0     offset 4     offset 8     offset 16
        // ref points directly to this slice's bytes[0]; ref - offset - 16 = A_ptr
        let ptr = unsafe { pool_b_alloc() };
        unsafe {
            (ptr as *mut u32).write(3u32); // rc=1, type=1
            (ptr.add(4) as *mut u32).write(sub_len as u32);
            *(ptr.add(8) as *mut *const u8) = new_ref;
            (ptr.add(16) as *mut u32).write(new_offset as u32);
        }
        Value(TAG_STR_BITS | (ptr as u64 & PAYLOAD_MASK))
    }

    // Shared allocator for the tuple pool header.  Writes Vec<Value> at offset 0
    // and the unique obj_id at offset 24, then tags with the supplied tag bits.
    //
    // Tuple is the only TAG_*_BITS payload that still uses this 32-byte slab
    // layout.  List moved to an `Rc<ListInner>` payload in #305 to make
    // `Value::clone` an alias rather than a deep copy.
    unsafe fn alloc_seq_hdr(tag_bits: u64, v: Vec<Value>, obj_id: u64) -> Self {
        let hdr = unsafe { pool_vec_hdr_alloc() };
        unsafe {
            std::ptr::write(hdr as *mut Vec<Value>, v);
            std::ptr::write(hdr.add(24) as *mut u64, obj_id);
        }
        Value(tag_bits | (hdr as u64 & PAYLOAD_MASK))
    }

    /// Construct a new `list` Value.  Storage is an `Rc<ListInner>` so that
    /// `Value::clone` shares the backing — matching Python's reference
    /// semantics for mutable containers (#305).
    pub fn list(v: Vec<Value>) -> Self {
        let inner = Rc::new(ListInner {
            items: RefCell::new(v),
            obj_id: next_obj_id(),
        });
        unsafe { Self::list_from_rc(inner) }
    }

    /// Construct a list Value from an existing `Rc<ListInner>` — used when
    /// multiple Values must share the same backing list (e.g. cloning).
    /// Caller is responsible for incrementing the strong count *before*
    /// calling this if they want a logical alias rather than a move.
    ///
    /// SAFETY: consumes one strong-count reference from `rc`.  The matching
    /// drop happens in `Drop for Value` when `TAG_LIST` is observed.
    unsafe fn list_from_rc(rc: Rc<ListInner>) -> Self {
        let raw = Rc::into_raw(rc);
        Value(TAG_LIST_BITS | (raw as u64 & PAYLOAD_MASK))
    }

    pub fn tuple(mut v: Vec<Value>) -> Self {
        // Small-tuple fast path (#281): route 2- and 3-element tuples through
        // `Opaque::SmallTuple2/3` so the backing `Vec<Value>` heap allocation
        // is avoided.  These shapes dominate hot sites (`dict.items()`,
        // `enumerate()`, `divmod()`, `str.partition()`, …).
        match v.len() {
            2 => {
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple2 {
                    items: [a, b],
                    obj_id: next_obj_id(),
                })
            }
            3 => {
                let c = v.pop().unwrap();
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple3 {
                    items: [a, b, c],
                    obj_id: next_obj_id(),
                })
            }
            _ => unsafe { Self::alloc_seq_hdr(TAG_TUPLE_BITS, v, next_obj_id()) },
        }
    }

    fn tuple_with_id(v: Vec<Value>, obj_id: u64) -> Self {
        unsafe { Self::alloc_seq_hdr(TAG_TUPLE_BITS, v, obj_id) }
    }

    pub fn dict(d: IndexMap<PyKey, Value>) -> Self {
        Value::opaque(Opaque::Dict(Rc::new(RefCell::new(d))))
    }

    pub fn set(s: IndexSet<PyKey>) -> Self {
        Value::opaque(Opaque::Set(Rc::new(SetInner {
            items: RefCell::new(s),
            obj_id: next_obj_id(),
        })))
    }

    pub fn bytes(b: Vec<u8>) -> Self {
        Value::opaque(Opaque::Bytes(Rc::new(b)))
    }

    pub fn complex(re: f64, im: f64) -> Self {
        Value::opaque(Opaque::Complex(re, im))
    }

    /// Construct a generic built-in object dispatched through the installed
    /// [`BuiltinTypeOps`] table.  `ops` must outlive the program (typically
    /// `&'static`); `state` is owned heap state of any concrete type.
    pub fn builtin_object(ops: &'static dyn BuiltinTypeOps, state: Box<dyn Any>) -> Self {
        Value::opaque(Opaque::BuiltinObject {
            ops,
            state: Rc::new(RefCell::new(state)),
        })
    }

    /// Construct a generic built-in object that shares state with an existing
    /// `BuiltinState` cell.  Used when multiple Values must reference the
    /// same underlying mutable state.
    pub fn builtin_object_shared(ops: &'static dyn BuiltinTypeOps, state: BuiltinState) -> Self {
        Value::opaque(Opaque::BuiltinObject { ops, state })
    }

    pub fn range(start: i64, stop: i64, step: i64) -> Self {
        Value::opaque(Opaque::Range { start, stop, step })
    }

    pub fn user_function(f: Rc<UserFunction>) -> Self {
        Value::opaque(Opaque::UserFunction(f))
    }

    /// Construct a built-in function value.  Stored as a `UserFunction` with
    /// `kind = Builtin(name)` so the function machinery is unified (one Opaque
    /// variant for both user and built-in functions).  The per-name
    /// `UserFunction` stub is interned in a thread-local cache so repeated
    /// calls don't reallocate it — equivalent in cost to the previous
    /// single-pointer payload.
    pub fn builtin_function(name: &'static str) -> Self {
        thread_local! {
            static CACHE: RefCell<HashMap<&'static str, Rc<UserFunction>>>
                = RefCell::new(HashMap::new());
        }
        let func = CACHE.with(|c| {
            if let Some(f) = c.borrow().get(name) {
                return Rc::clone(f);
            }
            let f = Rc::new(UserFunction {
                id: next_fn_id(),
                kind: UserFunctionKind::Builtin(name),
                name: name.to_string(),
                params: Vec::new(),
                local_names: Rc::new(HashSet::new()),
                local_index: Rc::new(HashMap::new()),
                global_names: Rc::new(HashSet::new()),
                nonlocal_names: Rc::new(HashSet::new()),
                env: Environment::new(None),
                is_pure: false,
                precompiled_code: None,
            });
            c.borrow_mut().insert(name, Rc::clone(&f));
            f
        });
        Value::opaque(Opaque::UserFunction(func))
    }

    /// The `NotImplemented` singleton.  Stored as a reserved NaN-box bit
    /// pattern so identity comparison is a single u64 equality check.
    pub fn not_implemented() -> Self {
        Value(NOT_IMPLEMENTED_BITS)
    }

    pub fn is_not_implemented(&self) -> bool {
        self.0 == NOT_IMPLEMENTED_BITS
    }

    pub fn py_class(c: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::PyClass(c))
    }

    pub fn py_instance(i: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::PyInstance(i))
    }

    pub fn py_module(m: Rc<RefCell<PyModule>>) -> Self {
        Value::opaque(Opaque::PyModule(m))
    }

    pub fn bound_method(function: Rc<UserFunction>, receiver: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::BoundMethod { function, receiver })
    }

    /// Wrap a function with a different `UserFunctionKind` tag.  Used by
    /// `@classmethod` / `@staticmethod`: produces a new UserFunction that
    /// shares everything but the kind tag.
    ///
    /// The wrapped function reuses the **original** `id` so the fn_cache and
    /// any other id-keyed caches share a single entry between the decorated
    /// and undecorated forms.  The function body and `is_pure` flag are
    /// identical (the kind tag only affects attribute-lookup-time binding,
    /// not execution), so cache hits across forms are correct.  See #303.
    pub fn with_function_kind(f: Rc<UserFunction>, kind: UserFunctionKind) -> Self {
        // Fast path: kind already matches — reuse the Rc directly.
        if f.kind == kind {
            return Value::opaque(Opaque::UserFunction(f));
        }
        let new_fn = UserFunction {
            id: f.id,
            kind,
            name: f.name.clone(),
            params: f.params.clone(),
            local_names: Rc::clone(&f.local_names),
            local_index: Rc::clone(&f.local_index),
            global_names: Rc::clone(&f.global_names),
            nonlocal_names: Rc::clone(&f.nonlocal_names),
            env: Rc::clone(&f.env),
            is_pure: f.is_pure,
            precompiled_code: f.precompiled_code.clone(),
        };
        Value::opaque(Opaque::UserFunction(Rc::new(new_fn)))
    }

    pub fn class_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::ClassMethod)
    }

    pub fn static_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::StaticMethod)
    }

    pub fn class_bound_method(function: Rc<UserFunction>, class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::ClassBoundMethod { function, class })
    }

    pub fn super_proxy(class: Rc<RefCell<PyClass>>, instance: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::SuperProxy { class, instance })
    }

    pub fn super_proxy_class(class: Rc<RefCell<PyClass>>, obj_class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::SuperProxyClass { class, obj_class })
    }

    /// Create a generator value.  `state` is the type-erased `GeneratorFrame`
    /// managed by the VM.
    pub fn generator(state: Box<dyn std::any::Any>) -> Self {
        Value::opaque(Opaque::Generator(Rc::new(RefCell::new(state))))
    }

    fn opaque(o: Opaque) -> Self {
        // SAFETY: `pool_opaque_alloc` returns a block sized/aligned for `Opaque`
        // (either a recycled one from this thread's free list or a fresh
        // `alloc(Layout::new::<Opaque>())`).  Writing through the cast pointer
        // initialises the slot; the matching `pool_opaque_dealloc` in `Drop` is
        // only invoked after `drop_in_place`, so no double-drop.  See #281.
        let ptr = unsafe { pool_opaque_alloc() as *mut Opaque };
        unsafe { std::ptr::write(ptr, o) };
        Value(TAG_OPAQUE_BITS | (ptr as u64 & PAYLOAD_MASK))
    }

    // ── Type checks ──────────────────────────────────────────────────────────

    pub fn is_none(&self) -> bool {
        self.0 == TAG_NONE_BITS
    }

    pub fn is_bool(&self) -> bool {
        top16(self.0) == TAG_BOOL
    }

    pub fn is_int(&self) -> bool {
        top16(self.0) == TAG_INT
            || (top16(self.0) == TAG_OPAQUE
                && matches!(unsafe { &*self.opaque_ptr() }, Opaque::PyBigInt(_)))
    }

    pub fn is_float(&self) -> bool {
        // Exclude the reserved positive-NaN sentinels (UNSET, NotImplemented)
        // whose `top16` falls in the float range but which aren't floats.
        if self.0 == NOT_IMPLEMENTED_BITS || self.0 == UNSET_BITS {
            return false;
        }
        top16(self.0) <= TAG_FLOAT_MAX
    }

    pub fn is_str(&self) -> bool {
        top16(self.0) == TAG_STR
    }

    pub fn is_tuple(&self) -> bool {
        if top16(self.0) == TAG_TUPLE {
            return true;
        }
        // Small tuples (2/3 elements) live in `Opaque::SmallTuple2/3` to
        // avoid the backing `Vec<Value>` heap allocation.  See #281.
        if top16(self.0) == TAG_OPAQUE {
            return matches!(
                unsafe { &*self.opaque_ptr() },
                Opaque::SmallTuple2 { .. } | Opaque::SmallTuple3 { .. }
            );
        }
        false
    }

    pub fn is_list(&self) -> bool {
        top16(self.0) == TAG_LIST
    }

    /// Returns a stable identity value for pool-allocated and Rc-shared types:
    /// - tuple: reads the monotonic obj_id stored at hdr+24
    /// - list: reads `obj_id` from the shared [`ListInner`]; aliased clones
    ///   (Rc-shared) all surface the same id, matching Python's `id()`
    ///   semantics for `b = a` aliasing (#305).
    /// - set: reads `obj_id` from the shared [`SetInner`] (same rationale).
    /// - str: uses the pool pointer address directly.
    ///
    /// Returns `None` for primitive types (callers handle those directly).
    pub fn value_id(&self) -> Option<i64> {
        // `as i64` wraps past 2^63; tracked separately, not specific to this
        // PR (tuple has the same shape).
        match top16(self.0) {
            TAG_TUPLE => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                Some(unsafe { *(hdr.add(24) as *const u64) } as i64)
            }
            TAG_LIST => Some(unsafe { self.list_inner() }.obj_id as i64),
            TAG_STR => Some((self.0 & PAYLOAD_MASK) as i64),
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                // Small-tuple variants stash a monotonic obj_id alongside their
                // inline payload so `id()` stays stable across clones; see #281.
                Opaque::SmallTuple2 { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SmallTuple3 { obj_id, .. } => Some(*obj_id as i64),
                // Sets are Rc-shared with an obj_id captured at construction;
                // aliased clones surface the same id (#305).
                Opaque::Set(rc) => Some(rc.obj_id as i64),
                // Dicts already share an Rc backing.  Surface the Rc pointer
                // address so `b = a; id(a) == id(b)` for dicts too (#305).
                Opaque::Dict(rc) => Some(Rc::as_ptr(rc) as i64),
                _ => None,
            },
            _ => None,
        }
    }

    // ── Private unsafe helpers ───────────────────────────────────────────────

    unsafe fn str_hdr(&self) -> *const u8 {
        (self.0 & PAYLOAD_MASK) as *const u8
    }

    unsafe fn str_as_str(&self) -> &str {
        unsafe {
            let hdr = self.str_hdr();
            let sub_len = *(hdr.add(4) as *const u32) as usize;
            let ref_ptr = *(hdr.add(8) as *const *const u8);
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(ref_ptr, sub_len))
        }
    }

    unsafe fn tuple_ptr(&self) -> *mut Vec<Value> {
        (self.0 & PAYLOAD_MASK) as *mut _
    }

    /// Raw pointer to the shared [`ListInner`] backing.  Caller must guarantee
    /// `self` is a TAG_LIST value.
    unsafe fn list_inner_ptr(&self) -> *const ListInner {
        (self.0 & PAYLOAD_MASK) as *const ListInner
    }

    /// Borrow the inner list header.  SAFETY: `self` must be a TAG_LIST value
    /// and the Rc must be live (which it is for any reachable `Value`).
    unsafe fn list_inner(&self) -> &ListInner {
        unsafe { &*self.list_inner_ptr() }
    }

    unsafe fn opaque_ptr(&self) -> *mut Opaque {
        (self.0 & PAYLOAD_MASK) as *mut _
    }

    // ── Public accessors ─────────────────────────────────────────────────────

    pub fn as_bool(&self) -> bool {
        (self.0 & 1) != 0
    }

    pub fn as_int_raw(&self) -> i64 {
        let raw = (self.0 & PAYLOAD_MASK) as i64;
        if self.0 & INT_SIGN_BIT != 0 {
            raw | !PAYLOAD_MASK as i64
        } else {
            raw
        }
    }

    pub fn as_float_raw(&self) -> f64 {
        f64::from_bits(self.0)
    }

    pub fn as_str(&self) -> Option<&str> {
        if self.is_str() {
            Some(unsafe { self.str_as_str() })
        } else {
            None
        }
    }

    /// Borrow the list's elements as a shared slice.
    ///
    /// SAFETY CONTRACT: the returned `&[Value]` borrows the underlying Vec
    /// through a raw pointer obtained from `RefCell::as_ptr()`; **no
    /// `Ref<...>` guard is held**.  This means the Rust aliasing model treats
    /// the read borrow as live for as long as the caller holds the returned
    /// reference, even though the `RefCell`'s internal counter is not
    /// incremented.
    ///
    /// Callers MUST NOT, while the returned reference is live:
    ///   1. obtain another borrow (mutable OR shared) on the same `Value`,
    ///   2. obtain a borrow on **any other `Value` that aliases the same
    ///      `Rc<ListInner>`** — list backing storage is Rc-shared after
    ///      #305, so `Value::clone` produces a second `Value` whose
    ///      `as_list[_mut]` would point at the same Vec,
    ///   3. call into code that may transitively re-enter this list (e.g.
    ///      user `__iter__`/`__hash__`).
    ///
    /// Single-threaded execution alone is NOT sufficient — the threat model
    /// here is intra-thread aliasing via Rc-clone, not data races.  When in
    /// doubt, materialise the read side into an owned `Vec<Value>` via
    /// `as_list().map(<[_]>::to_vec)` before reaching for a `&mut` borrow on
    /// any potentially-aliased Value.
    ///
    /// See `unalias_args_for_mutation` for the helper used at builtin
    /// dispatch sites to make this safe automatically.
    pub fn as_list(&self) -> Option<&[Value]> {
        if self.is_list() {
            let inner = unsafe { self.list_inner() };
            Some(unsafe { &*inner.items.as_ptr() })
        } else {
            None
        }
    }

    /// Borrow the list's elements as a mutable Vec.
    ///
    /// SAFETY CONTRACT: see [`Value::as_list`].  Same constraints apply, with
    /// the additional rule that the returned `&mut Vec<Value>` is
    /// exclusive — no other borrow (shared or mutable) on the same backing
    /// Rc<ListInner> may exist while it is live.  The `&mut self` receiver
    /// only blocks aliasing **through this Value**; a separately-rooted
    /// `Value` sharing the same `Rc` can still call `as_list()` or
    /// `kind()` and produce an aliased read borrow.  Builtin call sites that
    /// pass `args: Vec<Value>` alongside the receiver `&mut` MUST first
    /// unalias args that share identity with the receiver
    /// (`Value::value_id`); the helper `unalias_args_for_mutation` in the VM
    /// dispatch path does this.
    pub fn as_list_mut(&mut self) -> Option<&mut Vec<Value>> {
        if self.is_list() {
            let inner = unsafe { self.list_inner() };
            Some(unsafe { &mut *inner.items.as_ptr() })
        } else {
            None
        }
    }

    /// Borrow the tuple's elements as a slice.  Backs both the pool-allocated
    /// path (`TAG_TUPLE`) and the inline small-tuple path
    /// (`Opaque::SmallTuple2/3`); see #281.
    pub fn as_tuple(&self) -> Option<&[Value]> {
        if top16(self.0) == TAG_TUPLE {
            return Some(unsafe { &*self.tuple_ptr() });
        }
        if top16(self.0) == TAG_OPAQUE {
            match unsafe { &*self.opaque_ptr() } {
                Opaque::SmallTuple2 { items, .. } => return Some(&items[..]),
                Opaque::SmallTuple3 { items, .. } => return Some(&items[..]),
                _ => {}
            }
        }
        None
    }

    pub fn as_opaque(&self) -> Option<&Opaque> {
        if top16(self.0) == TAG_OPAQUE {
            Some(unsafe { &*self.opaque_ptr() })
        } else {
            None
        }
    }

    pub fn as_opaque_mut(&mut self) -> Option<&mut Opaque> {
        if top16(self.0) == TAG_OPAQUE {
            Some(unsafe { &mut *self.opaque_ptr() })
        } else {
            None
        }
    }

    /// Borrow the dict's IndexMap.
    ///
    /// SAFETY CONTRACT: see [`Value::as_list`].  Dict storage is
    /// `Rc<RefCell<IndexMap<...>>>` (Rc-shared since dict was the original
    /// reference type); the read borrow is unguarded via `RefCell::as_ptr`,
    /// so callers MUST NOT hold any other borrow (shared or mutable) on any
    /// Value that shares this `Rc` while the returned reference is live.
    pub fn as_dict(&self) -> Option<&IndexMap<PyKey, Value>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(unsafe { &*rc.as_ref().as_ptr() })
            } else {
                None
            }
        })
    }

    /// Borrow the dict's IndexMap as mutable.
    ///
    /// SAFETY CONTRACT: see [`Value::as_list_mut`].  Same constraints — the
    /// `&mut self` receiver only blocks aliasing through this Value; an
    /// Rc-cloned `Value` can still produce an aliased read borrow.  Call
    /// sites passing `args: Vec<Value>` alongside the `&mut` MUST unalias
    /// args that share identity with the receiver first.
    pub fn as_dict_mut(&mut self) -> Option<&mut IndexMap<PyKey, Value>> {
        self.as_opaque_mut().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(unsafe { &mut *rc.as_ref().as_ptr() })
            } else {
                None
            }
        })
    }

    /// Borrow the set's IndexSet.
    ///
    /// SAFETY CONTRACT: see [`Value::as_list`].  Set storage is `Rc<SetInner>`
    /// after #305; same Rc-aliasing concerns apply.
    pub fn as_set(&self) -> Option<&IndexSet<PyKey>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Set(rc) = o {
                Some(unsafe { &*rc.items.as_ptr() })
            } else {
                None
            }
        })
    }

    /// Borrow the set's IndexSet as mutable.
    ///
    /// SAFETY CONTRACT: see [`Value::as_list_mut`].  Same Rc-aliasing rules.
    pub fn as_set_mut(&mut self) -> Option<&mut IndexSet<PyKey>> {
        self.as_opaque_mut().and_then(|o| {
            if let Opaque::Set(rc) = o {
                Some(unsafe { &mut *rc.items.as_ptr() })
            } else {
                None
            }
        })
    }

    pub fn get_dict_rc(&self) -> Option<&Rc<RefCell<IndexMap<PyKey, Value>>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    /// Unified int accessor (handles inline i48 and PyBigInt that fits in i64)
    pub fn as_int(&self) -> Option<i64> {
        match top16(self.0) {
            TAG_INT => Some(self.as_int_raw()),
            TAG_OPAQUE => {
                if let Opaque::PyBigInt(rc) = unsafe { &*self.opaque_ptr() } {
                    rc.to_i64()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    // ── kind() — borrow-based view for pattern matching ──────────────────────

    pub fn kind(&self) -> ValueKind<'_> {
        // NotImplemented is encoded as a reserved NaN-box bit pattern; check
        // before the float arm so it doesn't get classified as a float NaN.
        if self.0 == NOT_IMPLEMENTED_BITS {
            return ValueKind::NotImplemented;
        }
        match top16(self.0) {
            t if t <= TAG_FLOAT_MAX => ValueKind::Float(self.as_float_raw()),
            TAG_NONE => ValueKind::None,
            TAG_BOOL => ValueKind::Bool(self.as_bool()),
            TAG_INT => ValueKind::Int(self.as_int_raw()),
            TAG_STR => ValueKind::Str(unsafe { self.str_as_str() }),
            TAG_TUPLE => ValueKind::Tuple(unsafe { &*self.tuple_ptr() }),
            // SAFETY: same single-threaded no-concurrent-borrow_mut invariant
            // as the dict/list cases below; see `as_list`.
            TAG_LIST => {
                let inner = unsafe { self.list_inner() };
                ValueKind::List(unsafe { &*inner.items.as_ptr() })
            }
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                Opaque::PyBigInt(rc) => {
                    if let Some(n) = rc.to_i64() {
                        ValueKind::Int(n)
                    } else {
                        ValueKind::BigInt(rc.as_ref())
                    }
                }
                // SAFETY: rc.as_ref().as_ptr() yields *mut IndexMap whose lifetime is
                // bounded by the Rc, which lives at least as long as this &self borrow.
                // No mutable borrow (borrow_mut) is held concurrently in our single-
                // threaded interpreter, so the raw-pointer alias is sound.
                Opaque::Dict(rc) => ValueKind::Dict(unsafe { &*rc.as_ref().as_ptr() }),
                // Sets share the same Rc<RefCell<...>> invariant after #305.
                Opaque::Set(rc) => ValueKind::Set(unsafe { &*rc.items.as_ptr() }),
                Opaque::Range { start, stop, step } => ValueKind::Range {
                    start: *start,
                    stop: *stop,
                    step: *step,
                },
                Opaque::UserFunction(f) => match f.kind {
                    UserFunctionKind::Builtin(name) => ValueKind::BuiltinFunction(name),
                    _ => ValueKind::UserFunction(f),
                },
                Opaque::PyClass(c) => ValueKind::PyClass(c),
                Opaque::PyInstance(i) => ValueKind::PyInstance(i),
                Opaque::PyModule(m) => ValueKind::PyModule(m),
                Opaque::BoundMethod { function, receiver } => {
                    ValueKind::BoundMethod { function, receiver }
                }
                Opaque::ClassBoundMethod { function, class } => {
                    ValueKind::ClassBoundMethod { function, class }
                }
                Opaque::SuperProxy { class, instance } => ValueKind::SuperProxy { class, instance },
                Opaque::SuperProxyClass { class, obj_class } => {
                    ValueKind::SuperProxyClass { class, obj_class }
                }
                Opaque::Generator(state) => ValueKind::Generator(state),
                Opaque::Bytes(rc) => ValueKind::Bytes(rc),
                Opaque::Complex(re, im) => ValueKind::Complex(*re, *im),
                // Inline small tuples surface as `ValueKind::Tuple(&[Value])`
                // so all existing match arms keep working without learning
                // about the new variant.  See #281.
                Opaque::SmallTuple2 { items, .. } => ValueKind::Tuple(&items[..]),
                Opaque::SmallTuple3 { items, .. } => ValueKind::Tuple(&items[..]),
                Opaque::BuiltinObject { ops, state } => {
                    ValueKind::BuiltinObject { ops: *ops, state }
                }
            },
            _ => unreachable!(),
        }
    }

    // ── Existing Value methods rewritten with kind() ─────────────────────────

    pub fn truthy(&self) -> bool {
        match self.kind() {
            ValueKind::Bool(v) => v,
            ValueKind::Int(v) => v != 0,
            ValueKind::BigInt(v) => !v.is_zero(),
            ValueKind::Float(v) => v != 0.0,
            ValueKind::Str(v) => !v.is_empty(),
            ValueKind::None => false,
            ValueKind::List(v) => !v.is_empty(),
            ValueKind::Dict(v) => !v.is_empty(),
            ValueKind::Set(v) => !v.is_empty(),
            ValueKind::Range { start, stop, step } => range_len(start, stop, step) > 0,
            ValueKind::UserFunction(_) => true,
            ValueKind::BuiltinFunction(_) => true,
            ValueKind::PyClass(_) => true,
            ValueKind::PyInstance(_) => true,
            ValueKind::BoundMethod { .. } => true,
            ValueKind::PyModule(_) => true,
            ValueKind::Tuple(v) => !v.is_empty(),
            ValueKind::ClassBoundMethod { .. } => true,
            ValueKind::SuperProxy { .. } => true,
            ValueKind::SuperProxyClass { .. } => true,
            ValueKind::Generator(_) => true,
            ValueKind::NotImplemented => true,
            // (NaN-box pattern handled by kind() dispatch above; included
            // in this match for completeness.)
            ValueKind::Bytes(b) => !b.is_empty(),
            ValueKind::Complex(re, im) => re != 0.0 || im != 0.0,
            ValueKind::BuiltinObject { ops, state } => ops.truthy(state),
        }
    }

    pub fn to_py_str(&self) -> String {
        match self.kind() {
            ValueKind::PyInstance(instance) if is_exception_instance(instance) => {
                exception_to_string(instance)
            }
            ValueKind::Str(s) => s.to_string(),
            _ => self.repr(),
        }
    }

    pub fn repr(&self) -> String {
        match self.kind() {
            ValueKind::Int(v) => v.to_string(),
            ValueKind::BigInt(v) => v.to_string(),
            ValueKind::Float(v) => format_float(v),
            ValueKind::Str(v) => format!("'{}'", escape_str(v)),
            ValueKind::Bool(v) => {
                if v {
                    "True".to_string()
                } else {
                    "False".to_string()
                }
            }
            ValueKind::None => "None".to_string(),
            ValueKind::List(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.repr())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{inner}]")
            }
            ValueKind::Dict(items) => {
                let mut out = String::new();
                out.push('{');
                for (i, (k, v)) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&key_repr(k));
                    out.push_str(": ");
                    out.push_str(&v.repr());
                }
                out.push('}');
                out
            }
            ValueKind::Set(items) => {
                if items.is_empty() {
                    return "set()".to_string();
                }
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("{{{inner}}}")
            }
            ValueKind::Range { start, stop, step } => {
                if step == 1 {
                    format!("range({start}, {stop})")
                } else {
                    format!("range({start}, {stop}, {step})")
                }
            }
            ValueKind::BuiltinFunction(name) => format!("<built-in function {name}>"),
            ValueKind::UserFunction(func) => match func.kind {
                UserFunctionKind::ClassMethod => format!("<classmethod '{}'>", func.name),
                UserFunctionKind::StaticMethod => format!("<staticmethod '{}'>", func.name),
                UserFunctionKind::Regular => format!("<function {}>", func.name),
                // Builtins are surfaced via `ValueKind::BuiltinFunction` by
                // `kind()`, so we never reach this arm — but the match is
                // total either way.
                UserFunctionKind::Builtin(name) => format!("<built-in function {name}>"),
            },
            ValueKind::PyClass(class) => {
                let name = class.borrow().name.clone();
                format!("<class '{name}'>")
            }
            ValueKind::PyInstance(instance) => {
                if is_exception_instance(instance) {
                    return exception_repr(instance);
                }
                let class_name = instance.borrow().class.borrow().name.clone();
                format!("<{class_name} object>")
            }
            ValueKind::BoundMethod { function, receiver } => {
                let class_name = receiver.borrow().class.borrow().name.clone();
                format!("<bound method {class_name}.{}>", function.name)
            }
            ValueKind::PyModule(m) => format!("<module '{}'>", m.borrow().name),
            ValueKind::Tuple(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.repr())
                    .collect::<Vec<_>>()
                    .join(", ");
                if items.len() == 1 {
                    format!("({inner},)")
                } else {
                    format!("({inner})")
                }
            }
            ValueKind::ClassBoundMethod { function, class } => {
                format!("<bound method {}.{}>", class.borrow().name, function.name)
            }
            ValueKind::SuperProxy { class, .. } => {
                format!("<super: <class '{}'>>", class.borrow().name)
            }
            ValueKind::SuperProxyClass { class, .. } => {
                format!("<super: <class '{}'>>", class.borrow().name)
            }
            ValueKind::Generator(_) => "<generator object>".to_string(),
            ValueKind::NotImplemented => "NotImplemented".to_string(),
            ValueKind::Bytes(rc) => bytes_repr(rc),
            ValueKind::Complex(re, im) => complex_repr(re, im),
            ValueKind::BuiltinObject { ops, state } => ops.repr(state),
        }
    }

    pub fn to_key(&self) -> Option<PyKey> {
        match self.kind() {
            ValueKind::Int(v) => Some(PyKey::Int(v)),
            ValueKind::BigInt(v) => v
                .to_i64()
                .map(PyKey::Int)
                .or_else(|| Some(PyKey::Str(v.to_string()))),
            ValueKind::Float(v) => Some(PyKey::Float(v.to_bits())),
            ValueKind::Str(v) => Some(PyKey::Str(v.to_string())),
            ValueKind::Bool(v) => Some(PyKey::Int(v as i64)),
            ValueKind::None => Some(PyKey::None),
            ValueKind::BuiltinObject { ops, state } => ops.to_key(state),
            _ => None,
        }
    }
}

// ── Clone ─────────────────────────────────────────────────────────────────────

impl Clone for Value {
    fn clone(&self) -> Self {
        match top16(self.0) {
            // Primitives: just copy bits
            t if t <= TAG_INT => Value(self.0),
            // Str
            TAG_STR => {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u32;
                unsafe {
                    // rc is stored in bits 31:1; increment by 2 (the type bit stays in bit 0).
                    // Saturate instead of wrapping: a saturated rc means we never free the
                    // backing buffer (acceptable memory leak for absurdly-shared strings).
                    let old = *hdr;
                    *hdr = old.saturating_add(2);
                } // rc++ (bits 31:1)
                Value(self.0) // same bits, 0 allocations
            }
            // Tuple — copy the stored obj_id so the clone shares the same identity
            TAG_TUPLE => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                let obj_id = unsafe { *(hdr.add(24) as *const u64) };
                let v = unsafe { &*self.tuple_ptr() };
                Value::tuple_with_id(v.clone(), obj_id)
            }
            // List — share the backing Rc<ListInner> with the original so that
            // mutations through any alias propagate to all clones (#305).  The
            // NaN-box pattern is reused directly; we only bump the strong
            // count to keep the Rc alive.  `obj_id` is inherent to the shared
            // `ListInner`, so identity (`id()`) is automatically stable.
            TAG_LIST => {
                unsafe {
                    Rc::increment_strong_count(self.list_inner_ptr());
                }
                Value(self.0)
            }
            // Opaque
            TAG_OPAQUE => {
                let o = unsafe { &*self.opaque_ptr() };
                Value::opaque(o.clone())
            }
            _ => unreachable!(),
        }
    }
}

// ── Drop ──────────────────────────────────────────────────────────────────────

impl Drop for Value {
    fn drop(&mut self) {
        match top16(self.0) {
            t if t <= TAG_INT => {} // primitives: no heap
            TAG_STR => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                let rc_type_ptr = hdr as *mut u32;
                *rc_type_ptr -= 2; // rc--
                if *rc_type_ptr >> 1 == 0 {
                    // rc reached 0
                    if *rc_type_ptr & 1 == 0 {
                        // Layout A: [rc_type:u32][sub_len:u32][ref:*mut u8][bytes...]
                        let len = *(hdr.add(4) as *const u32) as usize;
                        dealloc(hdr, Layout::from_size_align(16 + len, 8).unwrap());
                    } else {
                        // Layout B: [rc_type:u32][sub_len:u32][ref:*mut u8][offset:u32]
                        // A_ptr = ref - offset - 16
                        let ref_ptr = *(hdr.add(8) as *const *mut u8);
                        let offset = *(hdr.add(16) as *const u32) as usize;
                        let a_ptr = ref_ptr.sub(offset + 16);
                        *(a_ptr as *mut u32) -= 2; // A.rc--
                        if *(a_ptr as *const u32) >> 1 == 0 {
                            let root_len = *(a_ptr.add(4) as *const u32) as usize;
                            dealloc(a_ptr, Layout::from_size_align(16 + root_len, 8).unwrap());
                        }
                        pool_b_dealloc(hdr);
                    }
                }
            },
            TAG_TUPLE => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                std::ptr::drop_in_place(hdr as *mut Vec<Value>);
                pool_vec_hdr_dealloc(hdr);
            },
            // List — decrement the Rc strong count; the Rc layer drops the
            // underlying `ListInner` (and its `Vec<Value>`) when the count
            // reaches zero.  Pool allocations from the pre-#305 layout are
            // gone — the Rc-allocated block is freed by the standard
            // allocator, not the pool.
            TAG_LIST => unsafe {
                Rc::decrement_strong_count(self.list_inner_ptr());
            },
            TAG_OPAQUE => unsafe {
                // Matched allocator: `Value::opaque` allocates through
                // `pool_opaque_alloc`; drop the contained value in place and
                // hand the slab back to the same pool.  See #281.
                let ptr = self.opaque_ptr();
                std::ptr::drop_in_place(ptr);
                pool_opaque_dealloc(ptr as *mut u8);
            },
            _ => unreachable!(),
        }
    }
}

// ── PartialEq ─────────────────────────────────────────────────────────────────

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self.kind(), other.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => a == b,
            // Python: 1 == 1.0 is True
            (ValueKind::Int(a), ValueKind::Float(b)) => (a as f64) == b,
            (ValueKind::Float(a), ValueKind::Int(b)) => a == (b as f64),
            (ValueKind::Float(a), ValueKind::Float(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::BigInt(b)) => a == b,
            (ValueKind::BigInt(a), ValueKind::Int(b)) => *a == BigInt::from(b),
            (ValueKind::Int(a), ValueKind::BigInt(b)) => BigInt::from(a) == *b,
            (ValueKind::BigInt(a), ValueKind::Float(b)) => a.to_f64() == Some(b),
            (ValueKind::Float(a), ValueKind::BigInt(b)) => b.to_f64() == Some(a),
            (ValueKind::Str(a), ValueKind::Str(b)) => a == b,
            (ValueKind::Bool(a), ValueKind::Bool(b)) => a == b,
            // Python: True == 1 is True
            (ValueKind::Bool(a), ValueKind::Int(b)) => (a as i64) == b,
            (ValueKind::Int(a), ValueKind::Bool(b)) => a == (b as i64),
            // Python: True == 1.0 is True
            (ValueKind::Bool(a), ValueKind::Float(b)) => (a as u8 as f64) == b,
            (ValueKind::Float(a), ValueKind::Bool(b)) => a == (b as u8 as f64),
            (ValueKind::None, ValueKind::None) => true,
            (ValueKind::List(a), ValueKind::List(b)) => a == b,
            (ValueKind::Tuple(a), ValueKind::Tuple(b)) => a == b,
            (ValueKind::Dict(a), ValueKind::Dict(b)) => a == b,
            (ValueKind::Set(a), ValueKind::Set(b)) => a == b,
            (ValueKind::Bytes(a), ValueKind::Bytes(b)) => a.as_ref() == b.as_ref(),
            (ValueKind::Complex(ar, ai), ValueKind::Complex(br, bi)) => ar == br && ai == bi,
            (ValueKind::Int(n), ValueKind::Complex(br, bi)) => (n as f64) == br && bi == 0.0,
            (ValueKind::Complex(ar, ai), ValueKind::Int(n)) => ar == (n as f64) && ai == 0.0,
            (ValueKind::Float(f), ValueKind::Complex(br, bi)) => f == br && bi == 0.0,
            (ValueKind::Complex(ar, ai), ValueKind::Float(f)) => ar == f && ai == 0.0,
            (
                ValueKind::Range {
                    start: as_,
                    stop: ao,
                    step: at,
                },
                ValueKind::Range {
                    start: bs,
                    stop: bo,
                    step: bt,
                },
            ) => as_ == bs && ao == bo && at == bt,
            (ValueKind::BuiltinFunction(a), ValueKind::BuiltinFunction(b)) => a == b,
            (ValueKind::UserFunction(a), ValueKind::UserFunction(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyClass(a), ValueKind::PyClass(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
            (ValueKind::PyModule(a), ValueKind::PyModule(b)) => Rc::ptr_eq(a, b),
            (
                ValueKind::BoundMethod {
                    function: af,
                    receiver: ar,
                },
                ValueKind::BoundMethod {
                    function: bf,
                    receiver: br,
                },
            ) => Rc::ptr_eq(af, bf) && Rc::ptr_eq(ar, br),
            // Built-in objects dispatch equality through their ops trait so
            // pyrust-core never names a concrete built-in type.  Try both
            // directions so e.g. `frozenset == set` and `set == frozenset`
            // both reach the frozenset impl.
            (ValueKind::BuiltinObject { ops, state }, _) => ops.eq(state, other),
            (_, ValueKind::BuiltinObject { ops, state }) => ops.eq(state, self),
            _ => false,
        }
    }
}

// ── Display / Debug ───────────────────────────────────────────────────────────

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_py_str())
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.repr())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper free functions
// ─────────────────────────────────────────────────────────────────────────────

/// Replace any `Value` in `args` that shares the receiver's backing storage
/// (Rc-aliased list/set/dict) with an independent deep copy.  Used at builtin
/// dispatch sites before taking a `&mut` borrow on the receiver: the safety
/// contracts on [`Value::as_list_mut`], [`Value::as_set_mut`], and
/// [`Value::as_dict_mut`] forbid simultaneous borrows on aliased Values, so
/// e.g. `lst.extend(lst)` would otherwise create overlapping `&[Value]` and
/// `&mut Vec<Value>` references to the same storage even though no user-level
/// race occurs.
///
/// This is identity-based via [`Value::value_id`].  Primitives and types that
/// are not Rc-shared (tuple, str, etc.) flow through unchanged because their
/// `&[Value]` slices are stable for the borrow's lifetime regardless of
/// aliasing.
pub fn unalias_args_for_mutation(receiver: &Value, args: &mut [Value]) {
    let rid = match receiver.value_id() {
        Some(id) => id,
        None => return,
    };
    // Only the receiver kinds that are Rc-shared and mutated through `&mut`
    // need this treatment; bail early for other kinds.
    let is_aliasable = matches!(
        receiver.kind(),
        ValueKind::List(_) | ValueKind::Set(_) | ValueKind::Dict(_)
    );
    if !is_aliasable {
        return;
    }
    for arg in args.iter_mut() {
        if arg.value_id() == Some(rid) {
            *arg = match arg.kind() {
                ValueKind::List(items) => Value::list(items.to_vec()),
                ValueKind::Set(s) => Value::set(s.clone()),
                ValueKind::Dict(d) => Value::dict(d.clone()),
                // Same id but different kind shouldn't happen (id is derived
                // from the storage variant), but be defensive: leave as-is.
                _ => arg.clone(),
            };
        }
    }
}

/// Returns the Python built-in type name (e.g. `"list"`, `"str"`) for a
/// `Value`.  Used by error messages (`'X' object is not iterable`, attribute
/// errors), built-in method repr strings (`<built-in method append of list
/// object>`), and similar diagnostics.
///
/// This is the canonical implementation — every crate in the workspace
/// routes type-name lookup through this function so naming stays consistent.
/// The match is exhaustive over [`ValueKind`]; new variants must be added
/// here, not in per-crate copies.
pub fn builtin_type_name(value: &Value) -> &'static str {
    match value.kind() {
        ValueKind::None => "NoneType",
        ValueKind::Bool(_) => "bool",
        ValueKind::Int(_) | ValueKind::BigInt(_) => "int",
        ValueKind::Float(_) => "float",
        ValueKind::Str(_) => "str",
        ValueKind::List(_) => "list",
        ValueKind::Tuple(_) => "tuple",
        ValueKind::Dict(_) => "dict",
        ValueKind::Set(_) => "set",
        ValueKind::Range { .. } => "range",
        ValueKind::Bytes(_) => "bytes",
        ValueKind::Complex(_, _) => "complex",
        ValueKind::BuiltinFunction(_)
        | ValueKind::UserFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. } => "function",
        ValueKind::PyClass(_) => "type",
        ValueKind::PyInstance(_) => "object",
        ValueKind::PyModule(_) => "module",
        ValueKind::SuperProxy { .. } | ValueKind::SuperProxyClass { .. } => "super",
        ValueKind::Generator(_) => "generator",
        ValueKind::NotImplemented => "NotImplementedType",
        ValueKind::BuiltinObject { ops, .. } => ops.type_name(),
    }
}

/// Render a `bytes` value the way Python does (b'...' with escapes).
fn bytes_repr(bytes: &[u8]) -> String {
    // Choose a quote: if any single quote and no double quote, use double; else single.
    let has_single = bytes.contains(&b'\'');
    let has_double = bytes.contains(&b'"');
    let q = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push('b');
    out.push(q);
    for &b in bytes {
        match b {
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x5c => out.push_str("\\\\"),
            b'\'' if q == '\'' => out.push_str("\\'"),
            b'"' if q == '"' => out.push_str("\\\""),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(q);
    out
}

/// Format a single complex component the way CPython's repr does:
///   - integer-valued floats with |v| < 1e16 → `"3"` (no `.0`)
///   - |v| >= 1e16 → scientific notation `"1e+20"` (Python style)
///   - NaN / inf via `format_float`
///   - everything else → standard float repr
///
/// Python uses scientific notation for absolute values >= 1e16 (where i64
/// rounding would lose precision) and for very small non-zero values; we
/// mirror that boundary.
fn complex_component(v: f64) -> String {
    if !v.is_finite() {
        return format_float(v);
    }
    let abs = v.abs();
    if v == v.trunc() && abs < 1e16 {
        return format!("{}", v as i64);
    }
    if abs >= 1e16 || (abs != 0.0 && abs < 1e-4) {
        // Rust's `{:e}` produces "1e20"; CPython prints "1e+20". Patch the sign.
        let raw = format!("{v:e}");
        if let Some(idx) = raw.find('e') {
            let (mantissa, exp) = raw.split_at(idx);
            let exp = &exp[1..]; // skip 'e'
            if let Some(stripped) = exp.strip_prefix('-') {
                return format!("{mantissa}e-{stripped:0>2}");
            }
            return format!("{mantissa}e+{exp:0>2}");
        }
        return raw;
    }
    format_float(v)
}

/// Format a complex number the way Python does:
///   `1j`, `(2+3j)`, `(2-3j)`, `(-1+0j)`, etc.
fn complex_repr(re: f64, im: f64) -> String {
    let im_str = complex_component(im);
    if re == 0.0 && (1.0_f64).copysign(re) > 0.0 {
        return format!("{im_str}j");
    }
    let re_str = complex_component(re);
    let sep = if im < 0.0 || (im == 0.0 && im.is_sign_negative()) {
        ""
    } else {
        "+"
    };
    format!("({re_str}{sep}{im_str}j)")
}

fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::Float(v) => format_float(f64::from_bits(*v)),
        PyKey::Str(v) => format!("'{}'", escape_str(v)),
        PyKey::Bool(v) => {
            if *v {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        PyKey::None => "None".to_string(),
        PyKey::FrozenSet(items) => {
            if items.is_empty() {
                "frozenset()".to_string()
            } else {
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("frozenset({{{inner}}})")
            }
        }
    }
}

fn is_exception_instance(instance: &Rc<RefCell<PyInstance>>) -> bool {
    let class = Rc::clone(&instance.borrow().class);
    class_chain_contains_exception(&class)
}

fn class_chain_contains_exception(class: &Rc<RefCell<PyClass>>) -> bool {
    let (name, base) = {
        let borrowed = class.borrow();
        (borrowed.name.clone(), borrowed.base.clone())
    };
    if name == "Exception" {
        return true;
    }
    base.is_some_and(|base| class_chain_contains_exception(&base))
}

fn exception_args(instance: &Rc<RefCell<PyInstance>>) -> Vec<Value> {
    match instance.borrow().attrs.get("args").map(|v| v.kind()) {
        Some(ValueKind::List(args)) => args.to_vec(),
        _ => Vec::new(),
    }
}

fn format_exception_args(args: &[Value], repr_mode: bool) -> String {
    match args {
        [] => String::new(),
        [value] => {
            if repr_mode {
                value.repr()
            } else {
                value.to_py_str()
            }
        }
        _ => {
            let inner = args
                .iter()
                .map(|value| value.repr())
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
    }
}

fn exception_to_string(instance: &Rc<RefCell<PyInstance>>) -> String {
    let args = exception_args(instance);
    format_exception_args(&args, false)
}

fn exception_repr(instance: &Rc<RefCell<PyInstance>>) -> String {
    let class_name = instance.borrow().class.borrow().name.clone();
    let args = exception_args(instance);
    if args.is_empty() {
        format!("{class_name}()")
    } else {
        format!("{class_name}({})", format_exception_args(&args, true))
    }
}

fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\'', "\\'")
}

pub fn range_len(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            0
        } else {
            ((stop - start - 1) / step) + 1
        }
    } else if start <= stop {
        0
    } else {
        ((start - stop - 1) / (-step)) + 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PyError {
    Lex(String),
    Parse(String),
    Runtime(String),
    /// A named Python exception (e.g. "ValueError", "TypeError") raised from
    /// builtin code that cannot instantiate exception objects directly.
    /// The VM converts this to a proper PyInstance before propagating.
    ///
    /// `class_name` is a `Cow<'static, str>` so the overwhelmingly common
    /// case (a string literal like `"TypeError"`) is zero-allocation; rare
    /// dynamic class names (e.g. from user-defined exception types) can
    /// still be carried via `Cow::Owned`.
    Named(Cow<'static, str>, String), // (class_name, message)
    Raised(Value),
    /// Internal: used to unwind the VM call stack when a generator yields.
    /// Never observed outside the generator machinery.
    GeneratorYield(Value),
}

impl PyError {
    /// Convenience constructor for a named Python exception with a static
    /// class-name literal.  Avoids the per-call `"TypeError".to_string()`
    /// allocation that every error site would otherwise perform.
    #[inline]
    pub fn named(cls: &'static str, msg: impl Into<String>) -> Self {
        PyError::Named(Cow::Borrowed(cls), msg.into())
    }
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyError::Lex(s) => write!(f, "Lex error: {s}"),
            PyError::Parse(s) => write!(f, "Parse error: {s}"),
            PyError::Runtime(s) => write!(f, "Runtime error: {s}"),
            PyError::Named(cls, s) => write!(f, "{cls}: {s}"),
            PyError::Raised(value) => write!(f, "Uncaught exception: {}", value.repr()),
            PyError::GeneratorYield(value) => {
                write!(f, "internal: generator yielded {}", value.repr())
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, PyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_round_trips_through_kind() {
        let v = Value::not_implemented();
        assert!(v.is_not_implemented());
        assert!(matches!(v.kind(), ValueKind::NotImplemented));
        assert!(!v.is_unset());
    }

    #[test]
    fn not_implemented_is_not_classified_as_float() {
        // The NaN-box bit pattern shares the float top16 range; `kind()`
        // must intercept it before the float arm.  Regression guard for the
        // top16-vs-exact-bits caveat noted on `top16`.
        let v = Value::not_implemented();
        assert!(!matches!(v.kind(), ValueKind::Float(_)));
        assert!(!v.is_float());
    }

    #[test]
    fn not_implemented_repr_is_canonical() {
        assert_eq!(Value::not_implemented().repr(), "NotImplemented");
    }

    #[test]
    fn unset_and_not_implemented_are_distinct_patterns() {
        // Both use the positive-NaN sentinel family; they must not collide.
        let unset = Value::unset();
        let nimpl = Value::not_implemented();
        assert!(unset.is_unset());
        assert!(!unset.is_not_implemented());
        assert!(nimpl.is_not_implemented());
        assert!(!nimpl.is_unset());
    }

    /// Helper: build a minimal `UserFunction` for kind-wrapping tests.
    fn make_user_function() -> Rc<UserFunction> {
        Rc::new(UserFunction {
            id: next_fn_id(),
            kind: UserFunctionKind::Regular,
            name: "f".to_string(),
            params: Vec::new(),
            local_names: Rc::new(HashSet::new()),
            local_index: Rc::new(HashMap::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            env: Environment::new(None),
            is_pure: false,
            precompiled_code: None,
        })
    }

    fn extract_user_function(v: &Value) -> Rc<UserFunction> {
        match v.kind() {
            ValueKind::UserFunction(f) => Rc::clone(f),
            _ => panic!("expected UserFunction value"),
        }
    }

    #[test]
    fn with_function_kind_reuses_original_id() {
        // Regression: #303 — `@classmethod` / `@staticmethod` must reuse the
        // original `id` so they share `fn_cache` entries with the undecorated
        // form (and with each other), instead of allocating a fresh `id`
        // every time and doubling cache footprint.
        let original = make_user_function();
        let original_id = original.id;

        let cm = Value::class_method(Rc::clone(&original));
        let sm = Value::static_method(Rc::clone(&original));

        let cm_fn = extract_user_function(&cm);
        let sm_fn = extract_user_function(&sm);

        assert_eq!(cm_fn.id, original_id, "classmethod must reuse id");
        assert_eq!(sm_fn.id, original_id, "staticmethod must reuse id");
        assert_eq!(cm_fn.kind, UserFunctionKind::ClassMethod);
        assert_eq!(sm_fn.kind, UserFunctionKind::StaticMethod);
    }

    #[test]
    fn with_function_kind_idempotent_reuses_rc() {
        // When the requested kind already matches, return the same Rc — no
        // reallocation at all.
        let original = make_user_function();
        let wrapped = Value::with_function_kind(Rc::clone(&original), UserFunctionKind::Regular);
        let wrapped_fn = extract_user_function(&wrapped);
        assert!(
            Rc::ptr_eq(&original, &wrapped_fn),
            "kind-preserving wrap must reuse the original Rc"
        );
    }

    #[test]
    fn list_clone_shares_storage_for_bound_method_mutation() {
        // Regression test for #305.  `Value::clone` on a list must produce an
        // alias of the same backing storage, so that captured bound methods
        // (`m = lst.append; m(4)`) and simple aliasing (`b = a; b.append(x)`)
        // mutate the original list — matching CPython's reference semantics.
        let a = Value::list(vec![Value::int(1)]);
        let mut b = a.clone();
        b.as_list_mut()
            .expect("clone must still be a list")
            .push(Value::int(2));
        let items = a.as_list().expect("original must still be a list");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_int(), Some(1));
        assert_eq!(items[1].as_int(), Some(2));
    }

    #[test]
    fn list_clone_preserves_identity() {
        // `id(b) == id(a)` after `b = a` for list, matching CPython.
        let a = Value::list(vec![Value::int(1), Value::int(2)]);
        let b = a.clone();
        assert_eq!(a.value_id(), b.value_id());

        // Distinct list literals must NOT share identity.
        let c = Value::list(vec![Value::int(1), Value::int(2)]);
        assert_ne!(a.value_id(), c.value_id());
    }

    #[test]
    fn set_clone_shares_storage() {
        // Same Rc-sharing invariant as list, exercised through set's mutating
        // accessor.
        let a = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        let mut b = a.clone();
        b.as_set_mut()
            .expect("clone must still be a set")
            .insert(PyKey::Int(2));
        let items = a.as_set().expect("original must still be a set");
        assert!(items.contains(&PyKey::Int(1)));
        assert!(items.contains(&PyKey::Int(2)));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn set_mutation_through_original_visible_in_clone() {
        // Symmetric counterpart to `set_clone_shares_storage`: mutate via
        // the original Value, the clone (alias) sees it.  This pins both
        // directions of the Rc-shared backing post-#305.
        let mut a = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        let b = a.clone();
        a.as_set_mut()
            .expect("original must still be a set")
            .insert(PyKey::Int(2));
        let items = b.as_set().expect("clone must still be a set");
        assert!(items.contains(&PyKey::Int(1)));
        assert!(items.contains(&PyKey::Int(2)));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn set_clone_preserves_identity() {
        let a = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        let b = a.clone();
        assert_eq!(a.value_id(), b.value_id());

        let c = Value::set({
            let mut s = IndexSet::new();
            s.insert(PyKey::Int(1));
            s
        });
        assert_ne!(a.value_id(), c.value_id());
    }

    #[test]
    fn dict_clone_preserves_identity() {
        // Dict already used `Rc<RefCell<...>>` shared storage; #305 added an
        // `id()` surface for it via `value_id()`.  Pin the invariant.
        let a = Value::dict({
            let mut m = IndexMap::new();
            m.insert(PyKey::Str("k".to_string()), Value::int(1));
            m
        });
        let b = a.clone();
        assert_eq!(a.value_id(), b.value_id());

        let c = Value::dict({
            let mut m = IndexMap::new();
            m.insert(PyKey::Str("k".to_string()), Value::int(1));
            m
        });
        assert_ne!(a.value_id(), c.value_id());
    }
}
