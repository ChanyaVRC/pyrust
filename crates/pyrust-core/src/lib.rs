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
}

#[derive(Debug, Clone)]
pub struct UserFunction {
    /// Globally unique identity for fn_cache keying — stable across Rc drops/reallocations.
    pub id: u64,
    pub name: String,
    pub params: Vec<UserFunctionParam>,
    pub local_names: NameSet,
    pub local_index: Rc<HashMap<String, usize>>,
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
const TAG_BOOL_BITS: u64 = 0xFFFA_0000_0000_0000;
const TAG_INT_BITS: u64 = 0xFFFB_0000_0000_0000;
const TAG_STR_BITS: u64 = 0xFFFC_0000_0000_0000;
const TAG_TUPLE_BITS: u64 = 0xFFFD_0000_0000_0000;
const TAG_LIST_BITS: u64 = 0xFFFE_0000_0000_0000;
const TAG_OPAQUE_BITS: u64 = 0xFFFF_0000_0000_0000;

#[inline(always)]
fn top16(bits: u64) -> u16 {
    (bits >> 48) as u16
}

// ─────────────────────────────────────────────────────────────────────────────
// Opaque — heap-allocated types that don't fit in 48 bits
// ─────────────────────────────────────────────────────────────────────────────

pub enum Opaque {
    BigInt(i64),
    Dict(Rc<RefCell<IndexMap<PyKey, Value>>>),
    DictKeysView(Rc<RefCell<IndexMap<PyKey, Value>>>),
    DictValuesView(Rc<RefCell<IndexMap<PyKey, Value>>>),
    DictItemsView(Rc<RefCell<IndexMap<PyKey, Value>>>),
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
}

impl Clone for Opaque {
    fn clone(&self) -> Self {
        match self {
            Opaque::BigInt(n) => Opaque::BigInt(*n),
            Opaque::Dict(rc) => Opaque::Dict(Rc::clone(rc)),
            Opaque::DictKeysView(rc) => Opaque::DictKeysView(Rc::clone(rc)),
            Opaque::DictValuesView(rc) => Opaque::DictValuesView(Rc::clone(rc)),
            Opaque::DictItemsView(rc) => Opaque::DictItemsView(Rc::clone(rc)),
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
    Float(f64),
    Str(&'a str),
    List(&'a Vec<Value>),
    Tuple(&'a Vec<Value>),
    Dict(&'a IndexMap<PyKey, Value>),
    DictKeysView(&'a Rc<RefCell<IndexMap<PyKey, Value>>>),
    DictValuesView(&'a Rc<RefCell<IndexMap<PyKey, Value>>>),
    DictItemsView(&'a Rc<RefCell<IndexMap<PyKey, Value>>>),
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

    pub fn bool_(b: bool) -> Self {
        Value(TAG_BOOL_BITS | b as u64)
    }

    pub fn int(n: i64) -> Self {
        const MAX_I48: i64 = (1 << 47) - 1;
        const MIN_I48: i64 = -(1 << 47);
        if n >= MIN_I48 && n <= MAX_I48 {
            Value(TAG_INT_BITS | (n as u64 & PAYLOAD_MASK))
        } else {
            Value::opaque(Opaque::BigInt(n))
        }
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

    pub fn list(v: Vec<Value>) -> Self {
        let hdr = unsafe { pool_vec_hdr_alloc() };
        unsafe {
            std::ptr::write(hdr as *mut Vec<Value>, v);
            std::ptr::write(hdr.add(24) as *mut u64, next_obj_id());
        }
        Value(TAG_LIST_BITS | (hdr as u64 & PAYLOAD_MASK))
    }

    fn list_with_id(v: Vec<Value>, obj_id: u64) -> Self {
        let hdr = unsafe { pool_vec_hdr_alloc() };
        unsafe {
            std::ptr::write(hdr as *mut Vec<Value>, v);
            std::ptr::write(hdr.add(24) as *mut u64, obj_id);
        }
        Value(TAG_LIST_BITS | (hdr as u64 & PAYLOAD_MASK))
    }

    pub fn tuple(v: Vec<Value>) -> Self {
        let hdr = unsafe { pool_vec_hdr_alloc() };
        unsafe {
            std::ptr::write(hdr as *mut Vec<Value>, v);
            std::ptr::write(hdr.add(24) as *mut u64, next_obj_id());
        }
        Value(TAG_TUPLE_BITS | (hdr as u64 & PAYLOAD_MASK))
    }

    fn tuple_with_id(v: Vec<Value>, obj_id: u64) -> Self {
        let hdr = unsafe { pool_vec_hdr_alloc() };
        unsafe {
            std::ptr::write(hdr as *mut Vec<Value>, v);
            std::ptr::write(hdr.add(24) as *mut u64, obj_id);
        }
        Value(TAG_TUPLE_BITS | (hdr as u64 & PAYLOAD_MASK))
    }

    pub fn dict(d: IndexMap<PyKey, Value>) -> Self {
        Value::opaque(Opaque::Dict(Rc::new(RefCell::new(d))))
    }

    pub fn dict_keys_view(rc: Rc<RefCell<IndexMap<PyKey, Value>>>) -> Self {
        Value::opaque(Opaque::DictKeysView(rc))
    }

    pub fn dict_values_view(rc: Rc<RefCell<IndexMap<PyKey, Value>>>) -> Self {
        Value::opaque(Opaque::DictValuesView(rc))
    }

    pub fn dict_items_view(rc: Rc<RefCell<IndexMap<PyKey, Value>>>) -> Self {
        Value::opaque(Opaque::DictItemsView(rc))
    }

    pub fn set(s: IndexSet<PyKey>) -> Self {
        Value::opaque(Opaque::Set(s))
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

    fn opaque(o: Opaque) -> Self {
        let ptr = Box::into_raw(Box::new(o)) as u64;
        Value(TAG_OPAQUE_BITS | (ptr & PAYLOAD_MASK))
    }

    // ── Type checks ──────────────────────────────────────────────────────────

    pub fn is_none(&self) -> bool {
        self.0 == TAG_NONE_BITS
    }

    pub fn is_bool(&self) -> bool {
        top16(self.0) == 0xFFFA
    }

    pub fn is_int(&self) -> bool {
        top16(self.0) == 0xFFFB
            || (top16(self.0) == 0xFFFF
                && matches!(unsafe { &*self.opaque_ptr() }, Opaque::BigInt(_)))
    }

    pub fn is_float(&self) -> bool {
        top16(self.0) <= 0xFFF8
    }

    pub fn is_str(&self) -> bool {
        top16(self.0) == 0xFFFC
    }

    pub fn is_tuple(&self) -> bool {
        top16(self.0) == 0xFFFD
    }

    pub fn is_list(&self) -> bool {
        top16(self.0) == 0xFFFE
    }

    /// Returns a stable identity value for pool-allocated types:
    /// - list/tuple: reads the monotonic obj_id stored at hdr+24
    /// - str: uses the pool pointer address directly
    /// Returns `None` for all other types (Rc-based types use Rc::as_ptr).
    pub fn pool_ptr_id(&self) -> Option<i64> {
        match top16(self.0) {
            0xFFFD | 0xFFFE => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                Some(unsafe { *(hdr.add(24) as *const u64) } as i64)
            }
            0xFFFC => Some((self.0 & PAYLOAD_MASK) as i64),
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
        if top16(self.0) == 0xFFFF {
            Some(unsafe { &*self.opaque_ptr() })
        } else {
            None
        }
    }

    pub fn as_opaque_mut(&mut self) -> Option<&mut Opaque> {
        if top16(self.0) == 0xFFFF {
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

    pub fn get_dict_rc(&self) -> Option<&Rc<RefCell<IndexMap<PyKey, Value>>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    /// Unified int accessor (handles both inline i48 and Opaque::BigInt)
    pub fn as_int(&self) -> Option<i64> {
        match top16(self.0) {
            0xFFFB => Some(self.as_int_raw()),
            0xFFFF => {
                if let Opaque::BigInt(n) = unsafe { &*self.opaque_ptr() } {
                    Some(*n)
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
            t if t <= 0xFFF8 => ValueKind::Float(self.as_float_raw()),
            0xFFF9 => ValueKind::None,
            0xFFFA => ValueKind::Bool(self.as_bool()),
            0xFFFB => ValueKind::Int(self.as_int_raw()),
            0xFFFC => ValueKind::Str(unsafe { self.str_as_str() }),
            0xFFFD => ValueKind::Tuple(unsafe { &*self.tuple_ptr() }),
            0xFFFE => ValueKind::List(unsafe { &*self.list_ptr() }),
            0xFFFF => match unsafe { &*self.opaque_ptr() } {
                Opaque::BigInt(n) => ValueKind::Int(*n),
                // SAFETY: rc.as_ref().as_ptr() yields *mut IndexMap whose lifetime is
                // bounded by the Rc, which lives at least as long as this &self borrow.
                // No mutable borrow (borrow_mut) is held concurrently in our single-
                // threaded interpreter, so the raw-pointer alias is sound.
                Opaque::Dict(rc) => ValueKind::Dict(unsafe { &*rc.as_ref().as_ptr() }),
                Opaque::DictKeysView(rc) => ValueKind::DictKeysView(rc),
                Opaque::DictValuesView(rc) => ValueKind::DictValuesView(rc),
                Opaque::DictItemsView(rc) => ValueKind::DictItemsView(rc),
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
            },
            _ => unreachable!(),
        }
    }

    // ── Existing Value methods rewritten with kind() ─────────────────────────

    pub fn truthy(&self) -> bool {
        match self.kind() {
            ValueKind::Bool(v) => v,
            ValueKind::Int(v) => v != 0,
            ValueKind::Float(v) => v != 0.0,
            ValueKind::Str(v) => !v.is_empty(),
            ValueKind::None => false,
            ValueKind::List(v) => !v.is_empty(),
            ValueKind::Dict(v) => !v.is_empty(),
            ValueKind::DictKeysView(rc) => !rc.borrow().is_empty(),
            ValueKind::DictValuesView(rc) => !rc.borrow().is_empty(),
            ValueKind::DictItemsView(rc) => !rc.borrow().is_empty(),
            ValueKind::Set(v) => !v.is_empty(),
            ValueKind::Range { start, stop, step } => range_len(start, stop, step) > 0,
            ValueKind::UserFunction(_) => true,
            ValueKind::BuiltinFunction(_) => true,
            ValueKind::PyClass(_) => true,
            ValueKind::PyInstance(_) => true,
            ValueKind::BoundMethod { .. } => true,
            ValueKind::PyModule(_) => true,
            ValueKind::Tuple(v) => !v.is_empty(),
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
            ValueKind::Float(v) => {
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
            ValueKind::UserFunction(func) => format!("<function {}>", func.name),
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
            ValueKind::DictKeysView(rc) => {
                let map = rc.borrow();
                let keys: Vec<String> = map.keys().map(key_repr).collect();
                format!("dict_keys([{}])", keys.join(", "))
            }
            ValueKind::DictValuesView(rc) => {
                let map = rc.borrow();
                let vals: Vec<String> = map.values().map(|v| v.repr()).collect();
                format!("dict_values([{}])", vals.join(", "))
            }
            ValueKind::DictItemsView(rc) => {
                let map = rc.borrow();
                let items: Vec<String> = map
                    .iter()
                    .map(|(k, v)| format!("({}, {})", key_repr(k), v.repr()))
                    .collect();
                format!("dict_items([{}])", items.join(", "))
            }
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
        }
    }

    pub fn to_key(&self) -> Option<PyKey> {
        match self.kind() {
            ValueKind::Int(v) => Some(PyKey::Int(v)),
            ValueKind::Float(v) => Some(PyKey::Float(v.to_bits())),
            ValueKind::Str(v) => Some(PyKey::Str(v.to_string())),
            ValueKind::Bool(v) => Some(PyKey::Bool(v)),
            ValueKind::None => Some(PyKey::None),
            _ => None,
        }
    }
}

// ── Clone ─────────────────────────────────────────────────────────────────────

impl Clone for Value {
    fn clone(&self) -> Self {
        match top16(self.0) {
            // Primitives: just copy bits
            t if t <= 0xFFFB => Value(self.0),
            // Str
            0xFFFC => {
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
            0xFFFD => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                let obj_id = unsafe { *(hdr.add(24) as *const u64) };
                let v = unsafe { &*self.tuple_ptr() };
                Value::tuple_with_id(v.clone(), obj_id)
            }
            // List — copy the stored obj_id so the clone shares the same identity
            0xFFFE => {
                let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
                let obj_id = unsafe { *(hdr.add(24) as *const u64) };
                let v = unsafe { &*self.list_ptr() };
                Value::list_with_id(v.clone(), obj_id)
            }
            // Opaque
            0xFFFF => {
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
            t if t <= 0xFFFB => {} // primitives: no heap
            0xFFFC => unsafe {
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
            0xFFFD => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                std::ptr::drop_in_place(hdr as *mut Vec<Value>);
                pool_vec_hdr_dealloc(hdr);
            },
            0xFFFE => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                std::ptr::drop_in_place(hdr as *mut Vec<Value>);
                pool_vec_hdr_dealloc(hdr);
            },
            0xFFFF => unsafe {
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

fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::Float(v) => {
            let as_f = f64::from_bits(*v);
            if as_f.is_nan() {
                "nan".to_string()
            } else if as_f.is_infinite() {
                if as_f > 0.0 {
                    "inf".to_string()
                } else {
                    "-inf".to_string()
                }
            } else if as_f.fract() == 0.0 {
                format!("{as_f:.1}")
            } else {
                as_f.to_string()
            }
        }
        PyKey::Str(v) => format!("'{}'", escape_str(v)),
        PyKey::Bool(v) => {
            if *v {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        PyKey::None => "None".to_string(),
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
        Some(ValueKind::List(args)) => args.iter().cloned().collect(),
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
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyError::Lex(s) => write!(f, "Lex error: {s}"),
            PyError::Parse(s) => write!(f, "Parse error: {s}"),
            PyError::Runtime(s) => write!(f, "Runtime error: {s}"),
            PyError::Named(cls, s) => write!(f, "{cls}: {s}"),
            PyError::Raised(value) => write!(f, "Uncaught exception: {}", value.repr()),
        }
    }
}

pub type Result<T> = std::result::Result<T, PyError>;
