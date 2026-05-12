use std::alloc::{Layout, alloc, dealloc};
use std::any::Any;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserFunctionKind {
    #[default]
    Regular,
    ClassMethod,
    StaticMethod,
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
        Err(PyError::Runtime(format!(
            "'{}' object has no attribute '{}'",
            self.type_name(),
            name
        )))
    }

    fn call(
        &self,
        state: &BuiltinState,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let _ = (state, args, kwargs);
        Err(PyError::Runtime(format!(
            "'{}' object is not callable",
            self.type_name()
        )))
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let _ = (state, args, kwargs);
        Err(PyError::Runtime(format!(
            "'{}' object has no method '{}'",
            self.type_name(),
            name
        )))
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        let _ = state;
        Err(PyError::Runtime(format!(
            "'{}' object is not iterable",
            self.type_name()
        )))
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        let _ = state;
        None
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let _ = (state, key);
        Err(PyError::Runtime(format!(
            "'{}' object is not subscriptable",
            self.type_name()
        )))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let _ = (state, key, value);
        Err(PyError::Runtime(format!(
            "'{}' object does not support item assignment",
            self.type_name()
        )))
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let _ = (state, item);
        Err(PyError::Runtime(format!(
            "argument of type '{}' is not iterable",
            self.type_name()
        )))
    }

    /// Returns true if `name` is a method this type exposes.  Used by
    /// `hasattr(x, name)`.  Default checks via `call_method` — impls with
    /// fixed method tables should override for efficiency.
    fn has_method(&self, name: &str) -> bool {
        let _ = name;
        false
    }

    /// Returns true if this type is iterable.  Default: tries `iter_next`
    /// and observes whether the default "not iterable" error came back.
    /// Impls that override `iter_next` should override this too.
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

// ─────────────────────────────────────────────────────────────────────────────
// Opaque — heap-allocated types that don't fit in 48 bits
// ─────────────────────────────────────────────────────────────────────────────

pub enum Opaque {
    PyBigInt(Rc<BigInt>),
    Dict(Rc<RefCell<IndexMap<PyKey, Value>>>),
    Set(IndexSet<PyKey>),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    UserFunction(Rc<UserFunction>),
    BuiltinFunction(&'static str),
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
    /// A @property descriptor.  Each field is either a callable `Value` or
    /// `Value::none()` (meaning "not set").
    Property {
        fget: Rc<Value>,
        fset: Rc<Value>,
        fdel: Rc<Value>,
    },
    /// Intermediate callable returned by `prop.setter`, `prop.getter`, or
    /// `prop.deleter`.  Calling it with a function returns a new `Property`
    /// with that accessor replaced.
    PropertyAccessorPartial {
        /// Which slot to replace: 0 = fget, 1 = fset, 2 = fdel.
        slot: u8,
        fget: Rc<Value>,
        fset: Rc<Value>,
        fdel: Rc<Value>,
    },
    /// The `NotImplemented` singleton.  Returned by binary dunder methods to
    /// signal that the operation is not supported for the given operand types.
    NotImplemented,
    /// A method on a built-in type instance with the receiver bound.  Produced
    /// by `getattr(x, "method")` where `x` is a list/str/dict/tuple/set and
    /// `"method"` is one of its methods.  When called, dispatches through
    /// `pyrust-builtins` with `receiver` as `self`.
    BuiltinBoundMethod {
        name: Rc<String>,
        receiver: Value,
    },
    /// An immutable byte string.  Constructed via the `b"..."` literal or
    /// the `bytes(...)` builtin.  Stored behind `Rc` for cheap clones.
    Bytes(Rc<Vec<u8>>),
    /// A Python `complex` number stored as (real, imag).
    Complex(f64, f64),
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
            Opaque::Set(s) => Opaque::Set(s.clone()),
            Opaque::Range { start, stop, step } => Opaque::Range {
                start: *start,
                stop: *stop,
                step: *step,
            },
            Opaque::UserFunction(f) => Opaque::UserFunction(Rc::clone(f)),
            Opaque::BuiltinFunction(s) => Opaque::BuiltinFunction(s),
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
            Opaque::Property { fget, fset, fdel } => Opaque::Property {
                fget: Rc::clone(fget),
                fset: Rc::clone(fset),
                fdel: Rc::clone(fdel),
            },
            Opaque::PropertyAccessorPartial {
                slot,
                fget,
                fset,
                fdel,
            } => Opaque::PropertyAccessorPartial {
                slot: *slot,
                fget: Rc::clone(fget),
                fset: Rc::clone(fset),
                fdel: Rc::clone(fdel),
            },
            Opaque::NotImplemented => Opaque::NotImplemented,
            Opaque::BuiltinBoundMethod { name, receiver } => Opaque::BuiltinBoundMethod {
                name: Rc::clone(name),
                receiver: receiver.clone(),
            },
            Opaque::Bytes(rc) => Opaque::Bytes(Rc::clone(rc)),
            Opaque::Complex(re, im) => Opaque::Complex(*re, *im),
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
    List(&'a Vec<Value>),
    Tuple(&'a Vec<Value>),
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
    Property {
        fget: &'a Rc<Value>,
        fset: &'a Rc<Value>,
        fdel: &'a Rc<Value>,
    },
    PropertyAccessorPartial {
        slot: u8,
        fget: &'a Rc<Value>,
        fset: &'a Rc<Value>,
        fdel: &'a Rc<Value>,
    },
    NotImplemented,
    BuiltinBoundMethod {
        name: &'a Rc<String>,
        receiver: &'a Value,
    },
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

    // Shared allocator for list/tuple pool headers. Writes Vec<Value> at offset 0
    // and the unique obj_id at offset 24, then tags with the supplied tag bits.
    unsafe fn alloc_seq_hdr(tag_bits: u64, v: Vec<Value>, obj_id: u64) -> Self {
        let hdr = unsafe { pool_vec_hdr_alloc() };
        unsafe {
            std::ptr::write(hdr as *mut Vec<Value>, v);
            std::ptr::write(hdr.add(24) as *mut u64, obj_id);
        }
        Value(tag_bits | (hdr as u64 & PAYLOAD_MASK))
    }

    pub fn list(v: Vec<Value>) -> Self {
        unsafe { Self::alloc_seq_hdr(TAG_LIST_BITS, v, next_obj_id()) }
    }

    fn list_with_id(v: Vec<Value>, obj_id: u64) -> Self {
        unsafe { Self::alloc_seq_hdr(TAG_LIST_BITS, v, obj_id) }
    }

    pub fn tuple(v: Vec<Value>) -> Self {
        unsafe { Self::alloc_seq_hdr(TAG_TUPLE_BITS, v, next_obj_id()) }
    }

    fn tuple_with_id(v: Vec<Value>, obj_id: u64) -> Self {
        unsafe { Self::alloc_seq_hdr(TAG_TUPLE_BITS, v, obj_id) }
    }

    pub fn dict(d: IndexMap<PyKey, Value>) -> Self {
        Value::opaque(Opaque::Dict(Rc::new(RefCell::new(d))))
    }

    pub fn set(s: IndexSet<PyKey>) -> Self {
        Value::opaque(Opaque::Set(s))
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

    pub fn builtin_function(name: &'static str) -> Self {
        Value::opaque(Opaque::BuiltinFunction(name))
    }

    pub fn not_implemented() -> Self {
        Value::opaque(Opaque::NotImplemented)
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

    pub fn builtin_bound_method(name: impl Into<String>, receiver: Value) -> Self {
        Value::opaque(Opaque::BuiltinBoundMethod {
            name: Rc::new(name.into()),
            receiver,
        })
    }

    /// Wrap a function with a different `UserFunctionKind` tag.  Used by
    /// `@classmethod` / `@staticmethod`: produces a new UserFunction that
    /// shares everything but the kind tag.  The wrapped function gets a
    /// fresh `id` so it has its own identity for fn_cache keying.
    pub fn with_function_kind(f: Rc<UserFunction>, kind: UserFunctionKind) -> Self {
        let new_fn = UserFunction {
            id: next_fn_id(),
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

    /// Create a `@property` descriptor value.  Pass `Value::none()` for any
    /// accessor that is not set.
    pub fn property(fget: Value, fset: Value, fdel: Value) -> Self {
        Value::opaque(Opaque::Property {
            fget: Rc::new(fget),
            fset: Rc::new(fset),
            fdel: Rc::new(fdel),
        })
    }

    /// Returned by `prop.setter(fn)` — creates a new Property with fset replaced.
    pub fn property_setter_partial(fget: Value, fdel: Value) -> Self {
        Value::opaque(Opaque::PropertyAccessorPartial {
            slot: 1,
            fget: Rc::new(fget),
            fset: Rc::new(Value::none()),
            fdel: Rc::new(fdel),
        })
    }

    /// Returned by `prop.deleter(fn)` — creates a new Property with fdel replaced.
    pub fn property_deleter_partial(fget: Value, fset: Value) -> Self {
        Value::opaque(Opaque::PropertyAccessorPartial {
            slot: 2,
            fget: Rc::new(fget),
            fset: Rc::new(fset),
            fdel: Rc::new(Value::none()),
        })
    }

    /// Returned by `prop.getter(fn)` — creates a new Property with fget replaced.
    pub fn property_getter_partial(fset: Value, fdel: Value) -> Self {
        Value::opaque(Opaque::PropertyAccessorPartial {
            slot: 0,
            fget: Rc::new(Value::none()),
            fset: Rc::new(fset),
            fdel: Rc::new(fdel),
        })
    }

    fn opaque(o: Opaque) -> Self {
        let ptr = Box::into_raw(Box::new(o)) as u64;
        Value(TAG_OPAQUE_BITS | (ptr & PAYLOAD_MASK))
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
        top16(self.0) <= TAG_FLOAT_MAX
    }

    pub fn is_str(&self) -> bool {
        top16(self.0) == TAG_STR
    }

    pub fn is_tuple(&self) -> bool {
        top16(self.0) == TAG_TUPLE
    }

    pub fn is_list(&self) -> bool {
        top16(self.0) == TAG_LIST
    }

    /// Returns a stable identity value for pool-allocated types:
    /// - list/tuple: reads the monotonic obj_id stored at hdr+24
    /// - str: uses the pool pointer address directly
    ///   Returns `None` for Rc-based and primitive types (callers handle those directly).
    pub fn value_id(&self) -> Option<i64> {
        match top16(self.0) {
            TAG_TUPLE | TAG_LIST => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                Some(unsafe { *(hdr.add(24) as *const u64) } as i64)
            }
            TAG_STR => Some((self.0 & PAYLOAD_MASK) as i64),
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

    unsafe fn list_ptr(&self) -> *mut Vec<Value> {
        (self.0 & PAYLOAD_MASK) as *mut _
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

    pub fn as_list(&self) -> Option<&Vec<Value>> {
        if self.is_list() {
            Some(unsafe { &*self.list_ptr() })
        } else {
            None
        }
    }

    pub fn as_list_mut(&mut self) -> Option<&mut Vec<Value>> {
        if self.is_list() {
            Some(unsafe { &mut *self.list_ptr() })
        } else {
            None
        }
    }

    pub fn as_tuple(&self) -> Option<&Vec<Value>> {
        if self.is_tuple() {
            Some(unsafe { &*self.tuple_ptr() })
        } else {
            None
        }
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

    pub fn as_dict(&self) -> Option<&IndexMap<PyKey, Value>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                // SAFETY: same invariant as in kind() — no concurrent mutable borrow.
                Some(unsafe { &*rc.as_ref().as_ptr() })
            } else {
                None
            }
        })
    }

    pub fn as_dict_mut(&mut self) -> Option<&mut IndexMap<PyKey, Value>> {
        self.as_opaque_mut().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                // SAFETY: &mut self prevents any other alias to this Value while the
                // returned reference is live.  No concurrent borrow_mut in single-threaded use.
                Some(unsafe { &mut *rc.as_ref().as_ptr() })
            } else {
                None
            }
        })
    }

    pub fn as_set_mut(&mut self) -> Option<&mut IndexSet<PyKey>> {
        self.as_opaque_mut().and_then(|o| {
            if let Opaque::Set(s) = o {
                Some(s)
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
        match top16(self.0) {
            t if t <= TAG_FLOAT_MAX => ValueKind::Float(self.as_float_raw()),
            TAG_NONE => ValueKind::None,
            TAG_BOOL => ValueKind::Bool(self.as_bool()),
            TAG_INT => ValueKind::Int(self.as_int_raw()),
            TAG_STR => ValueKind::Str(unsafe { self.str_as_str() }),
            TAG_TUPLE => ValueKind::Tuple(unsafe { &*self.tuple_ptr() }),
            TAG_LIST => ValueKind::List(unsafe { &*self.list_ptr() }),
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
                Opaque::Set(s) => ValueKind::Set(s),
                Opaque::Range { start, stop, step } => ValueKind::Range {
                    start: *start,
                    stop: *stop,
                    step: *step,
                },
                Opaque::UserFunction(f) => ValueKind::UserFunction(f),
                Opaque::BuiltinFunction(s) => ValueKind::BuiltinFunction(s),
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
                Opaque::Property { fget, fset, fdel } => ValueKind::Property { fget, fset, fdel },
                Opaque::PropertyAccessorPartial {
                    slot,
                    fget,
                    fset,
                    fdel,
                } => ValueKind::PropertyAccessorPartial {
                    slot: *slot,
                    fget,
                    fset,
                    fdel,
                },
                Opaque::NotImplemented => ValueKind::NotImplemented,
                Opaque::BuiltinBoundMethod { name, receiver } => {
                    ValueKind::BuiltinBoundMethod { name, receiver }
                }
                Opaque::Bytes(rc) => ValueKind::Bytes(rc),
                Opaque::Complex(re, im) => ValueKind::Complex(*re, *im),
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
            ValueKind::Property { .. } => true,
            ValueKind::PropertyAccessorPartial { .. } => true,
            ValueKind::NotImplemented => true,
            ValueKind::BuiltinBoundMethod { .. } => true,
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
            ValueKind::Property { .. } => "<property object>".to_string(),
            ValueKind::PropertyAccessorPartial { .. } => "<property accessor partial>".to_string(),
            ValueKind::NotImplemented => "NotImplemented".to_string(),
            ValueKind::BuiltinBoundMethod { name, receiver } => {
                format!(
                    "<built-in method {name} of {} object>",
                    builtin_type_name(receiver)
                )
            }
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
            // List — copy the stored obj_id so the clone shares the same identity
            TAG_LIST => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                let obj_id = unsafe { *(hdr.add(24) as *const u64) };
                let v = unsafe { &*self.list_ptr() };
                Value::list_with_id(v.clone(), obj_id)
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
            TAG_LIST => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                std::ptr::drop_in_place(hdr as *mut Vec<Value>);
                pool_vec_hdr_dealloc(hdr);
            },
            TAG_OPAQUE => unsafe {
                drop(Box::from_raw(self.opaque_ptr()));
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

/// Returns the built-in type name (e.g. "list", "str") for use in
/// repr strings like "<built-in method append of list object>".
pub fn builtin_type_name(value: &Value) -> &'static str {
    match value.kind() {
        ValueKind::Str(_) => "str",
        ValueKind::List(_) => "list",
        ValueKind::Tuple(_) => "tuple",
        ValueKind::Dict(_) => "dict",
        ValueKind::Set(_) => "set",
        ValueKind::Bytes(_) => "bytes",
        ValueKind::Complex(_, _) => "complex",
        ValueKind::Int(_) | ValueKind::BigInt(_) => "int",
        ValueKind::Float(_) => "float",
        ValueKind::Bool(_) => "bool",
        ValueKind::None => "NoneType",
        ValueKind::BuiltinObject { ops, .. } => ops.type_name(),
        _ => "object",
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
    Named(String, String), // (class_name, message)
    Raised(Value),
    /// Internal: used to unwind the VM call stack when a generator yields.
    /// Never observed outside the generator machinery.
    GeneratorYield(Value),
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
