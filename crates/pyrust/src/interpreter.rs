use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use indexmap::IndexMap;
use smallvec::smallvec;

use crate::ast::{AssignTarget, BinaryOp, Expr, Stmt, UnaryOp};
use crate::bytecode::{FnCode, GLOBAL_CACHE_EMPTY};
use crate::error::{PyError, Result};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{
    EnvRef, Environment, PyBigInt, PyBigIntSign, PyClass, PyInstance, PyKey, PyModule, PyPow,
    PyToPrimitive, PyZero, StrKey, UserFunction, UserFunctionKind, UserFunctionParam, Value,
    ValueKind, intern_string, intern_string_value,
};

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;

/// A memoisation key that is type-aware: `Float(1.0)` and `Int(1)` are
/// equal as `PyKey` (dict/set semantics) but must be distinct memo keys
/// because a pure function may branch on `type(x)`.
///
/// We include the `PyKey` variant discriminant alongside the `PyKey` value so
/// that two calls are only considered cache-equivalent when both the runtime
/// type *and* the value agree.
#[derive(Clone)]
pub(crate) struct MemoKey(pub(crate) PyKey);

/// Type-aware equality for a single `PyKey`, borrowing both sides to avoid
/// unnecessary clones in the hot tuple/frozenset element comparison path.
fn memo_key_eq(a: &PyKey, b: &PyKey) -> bool {
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return false;
    }
    match (a, b) {
        (PyKey::Tuple(xs), PyKey::Tuple(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| memo_key_eq(x, y))
        }
        (PyKey::FrozenSet(xs), PyKey::FrozenSet(ys)) => {
            // FrozenSet elements are stored in sorted canonical order by the
            // PyKey constructor; element-wise comparison suffices.
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| memo_key_eq(x, y))
        }
        _ => a == b,
    }
}

/// Type-aware hashing for a single `PyKey`, borrowing the key to avoid
/// unnecessary clones in the hot tuple/frozenset element hashing path.
fn hash_memo_key<H: std::hash::Hasher>(k: &PyKey, state: &mut H) {
    use std::hash::Hash;
    std::mem::discriminant(k).hash(state);
    match k {
        PyKey::Tuple(items) => {
            items.len().hash(state);
            for elem in items {
                hash_memo_key(elem, state);
            }
        }
        PyKey::FrozenSet(items) => {
            for elem in items {
                hash_memo_key(elem, state);
            }
        }
        _ => k.hash(state),
    }
}

impl PartialEq for MemoKey {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        memo_key_eq(&self.0, &other.0)
    }
}

impl Eq for MemoKey {}

impl std::hash::Hash for MemoKey {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        hash_memo_key(&self.0, state);
    }
}

type FnCache = HashMap<(u64, Vec<MemoKey>), Value>;

const MAX_CALL_DEPTH: usize = 1000;
const ENV_POOL_MAX: usize = 64;

/// Pre-resolved table of built-in exception classes.
///
/// Initialised lazily on the first call to `get` — the exception hierarchy is
/// built via the thread-local `EXC_CLASS_CACHE` only when actually needed.
/// Scripts that never raise or catch exceptions pay zero startup cost.
pub(crate) struct ExcClasses(RefCell<Option<HashMap<&'static str, Rc<RefCell<PyClass>>>>>);

impl ExcClasses {
    fn uninitialized() -> Self {
        ExcClasses(RefCell::new(None))
    }

    /// Look up a built-in exception class by its `'static` name.
    /// Initialises the class table on the very first call.
    #[inline]
    pub(crate) fn get(&self, name: &'static str) -> Option<Rc<RefCell<PyClass>>> {
        let mut guard = self.0.borrow_mut();
        let map = guard.get_or_insert_with(build_exc_class_map);
        map.get(name).map(Rc::clone)
    }
}

pub struct Interpreter {
    pub(crate) env: EnvRef,
    active_exception: Option<Value>,
    /// Stack of currently-handled exceptions (PEP 3134 `__context__`).
    /// An entry is pushed when an exception handler catches an exception
    /// (matching CPython's per-frame "exc_info" stack), and popped when
    /// the handler exits — either normally via `EndExcept`, or because
    /// a new exception propagates out of the handler body.
    ///
    /// The top of this stack is the exception whose handler we are
    /// currently inside, and it is the value attached as `__context__`
    /// to any *new* exception raised inside that handler body.
    ///
    /// Note: this differs from `active_exception`, which can be cleared
    /// transiently while the handler body is running (e.g. by an inner
    /// `EndExcept`).  The stack lets us restore the outer context.
    handled_exc_stack: Vec<Value>,
    script_dir: Option<PathBuf>,
    module_cache: ModuleCache,
    env_pool: Vec<EnvRef>,
    fn_cache: FnCache,
    /// Reusable argument buffer for VM Call instructions — avoids a per-call
    /// heap allocation in the common (non-recursive) case.
    call_arg_buf: Vec<ExpandedCallArg>,
    /// Reusable positional-args buffer for the builtin bound-method dispatch
    /// path — avoids a per-call heap allocation on the hot path (issue #276).
    /// Pattern: `std::mem::take`, clear, fill, use (borrow or drain), then
    /// restore so subsequent calls reuse the grown capacity.
    bound_method_pos_buf: Vec<Value>,
    /// Reusable scratch buffer for building fn_cache probe keys — avoids a
    /// per-probe heap allocation in CallMemo's cache-hit path.
    key_scratch: Vec<MemoKey>,
    /// Stack of class-namespace store-order lists, pushed by `MakeClass` before
    /// running a class body and popped after.  Each entry tracks the slot numbers
    /// for class-body locals in the **order stores actually executed** at
    /// runtime — CPython's `__dict__` ordering rule.  A stack (not a single Vec)
    /// supports nested `class A: class B: ...` bodies cleanly.
    pub(crate) class_store_order: Vec<Vec<u32>>,
    /// Stack of active VM frame views (issue #389).  Pushed before each
    /// `run_bytecode` invocation and popped immediately afterwards by:
    ///   * `try_exec_vm_script_with_index` (kind = `Script`), and
    ///   * `call_user_function_expanded`'s register-VM tier (kind =
    ///     `Function`).
    ///
    /// Each entry holds a raw pointer to the active frame's register
    /// file plus the name -> slot mapping the compiler emitted.
    /// Built-ins like `globals()` / `locals()` consult this stack to
    /// surface names that would otherwise live only in registers:
    ///   * `globals()` walks down to the BOTTOM-most `Script` entry to
    ///     find the module's fastlocals (e.g. top-level `x = 5`).
    ///   * `locals()` reads the TOP entry — innermost function frame,
    ///     or the script frame at module scope.
    ///
    /// Safety: each raw pointer is only dereferenced while the frame
    /// is still on the VM stack, i.e. inside the same VM entry that
    /// pushed it.  The push/pop invariant is maintained by:
    ///   * `program.rs::try_exec_vm_script_with_index` (`Script`)
    ///   * `calls.rs::call_user_function_expanded` — both the simple
    ///     and variadic user-function paths (`Function`)
    ///   * `vm.rs::resume_generator_with_exc` — each generator resume
    ///     (`Function`; the regs pointer comes from the heap-allocated
    ///     `GeneratorFrame::regs`, stable across yields)
    /// Class bodies (`Insn::MakeClass`) publish a `FrameKind::Class` view
    /// so that `locals()` inside a class body returns the partially-built
    /// class attrs dict (issue #487).
    pub(crate) vm_frame_views: Vec<VmFrameView>,
    /// Recursion stack for `values_user_eq` cycle detection (issue
    /// #436).  Each entry is the ordered `(value_id(lhs), value_id(rhs))`
    /// of a container pair currently being compared element-wise.
    /// Encountering the same pair again means the recursion has hit a
    /// cycle (e.g. `a.append(a); b.append(b); a == b`); the recursion
    /// short-circuits to `true` instead of blowing the stack.
    ///
    /// **Intentional divergence from CPython:** CPython raises
    /// `RecursionError` when the prefix is all-equal and the back-edge
    /// is reached.  pyrust instead returns `true` (the same policy as
    /// `Value::eq`'s `EqGuard`).  The parity fixture
    /// `test_container_eq_dispatches_eq.py` documents and tests only
    /// the prefix-differs case (which is deterministic in both
    /// implementations); the all-equal-cycle case is explicitly excluded
    /// from parity testing because CPython's behaviour there is
    /// `RecursionError`, not `True` or `False`.
    ///
    /// Lives on the interpreter (rather than a `thread_local!`) because
    /// the helper is only reachable through `&mut self`; the field
    /// avoids the per-recursive-call thread-local borrow.
    pub(crate) eq_in_progress: Vec<(i64, i64)>,
    /// Pre-resolved table of built-in exception classes, populated once
    /// at interpreter startup by [`Interpreter::default`] after
    /// `install_exception_builtins` registers the classes into `env`.
    ///
    /// The table maps the class name (`"TypeError"`, `"ValueError"`, …)
    /// to the `Rc<RefCell<PyClass>>` stored in `env`.  Using this cache
    /// the VM dispatch loop can construct [`PyError::Class`] errors
    /// directly — no env-hierarchy name lookup at the raise site.
    ///
    /// [`ExcClasses`] is a newtype that exposes a `get(&str)` method so
    /// callers stay legible and the HashMap detail stays local.
    pub(crate) exc_classes: ExcClasses,
    /// The persistent module globals dict — the single `Value::dict` that
    /// `globals()` returns on every call (issue #706).
    ///
    /// `globals()` returns `module_globals_dict.clone()`, which shares the
    /// same `Rc<RefCell<IndexMap<...>>>` — callers that mutate the returned
    /// dict are mutating this backing store directly, and `LoadGlobal`
    /// checks this dict as a fallback so `globals()["x"] = val` mutations
    /// are visible as the global `x` immediately.
    pub(crate) module_globals_dict: Value,
    /// Set to `true` the first time `globals()` (or `locals()` at module
    /// scope) is called.  While `false`, `assign_name` skips the eager
    /// `module_globals_dict` write — this is the common case for scripts
    /// that never call `globals()`, and the main source of the regression
    /// introduced by PR #810.  Once `true`, `assign_name` resumes writing
    /// to the dict so the returned live view stays in sync (issue #810).
    pub(crate) globals_accessed: bool,
    /// Monotonic version counter for the global environment.
    ///
    /// Incremented on every write or delete to the module-level `env.values`
    /// (via `assign_name` at module scope or `global`-declared names in
    /// functions) and on explicit `del` of module-level names.
    ///
    /// `LoadGlobal` caches the resolved value alongside this counter in
    /// `FnCode::global_cache[name_idx]`.  A cache entry is valid when its
    /// stored version equals the current `global_env_version`; on mismatch
    /// the slow env-chain lookup runs and the entry is refreshed.  Builtin
    /// resolutions are cached with the current version as well, so that a
    /// subsequent module-level assignment of the same name (e.g. `len = fn`)
    /// correctly invalidates the cached builtin entry.
    ///
    /// Wrapped in `Cell<u32>` so that `assign_name` (which takes `&self`
    /// for consistency with its callers) can increment it via interior
    /// mutability without requiring `&mut Interpreter` at call sites.
    ///
    /// Note: `globals()["x"] = y` mutations (writing directly to
    /// `module_globals_dict` without going through `assign_name`) do not
    /// increment this counter.  Values surfaced only through the dict
    /// fallback path are therefore not cached — they fall through to the
    /// slow lookup on every `LoadGlobal` execution.  This is acceptable
    /// because the dict-only path is already guarded by `globals_accessed`
    /// and is not on the hot path of normal code.
    pub(crate) global_env_version: Cell<u32>,
}

/// Discriminator for `VmFrameView`: script-level (module-scope) vs.
/// function-level vs. class-body frames.  `globals()` and `locals()` need to
/// tell these apart — `globals()` always wants the script-level view,
/// `locals()` wants whichever is innermost.  `Class` frames expose the
/// in-progress class-body register file so `locals()` inside a class body
/// returns the partially-built attrs dict (issue #487).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameKind {
    Script,
    Function,
    /// Class-body evaluation frame.  The `regs_ptr`/`regs_len` point at the
    /// class body's fastlocal register file; `local_index` maps names to
    /// slots exactly as for `Function` frames.  `nonlocal_names` and `env`
    /// are always `None` (class bodies have no nonlocal semantics).
    Class,
}

/// Snapshot of a VM frame's register file and name index, used by
/// `globals()` / `locals()` / `vars()` to surface names that live in
/// registers rather than `env.values` (issue #389).
pub(crate) struct VmFrameView {
    pub(crate) kind: FrameKind,
    /// Non-null raw pointer to the active frame's register file.  Valid only
    /// while the corresponding `run_bytecode` invocation is on the call stack
    /// — `Interpreter::vm_frame_views` is pushed/popped in lock-step with
    /// that lifetime by the caller.
    ///
    /// # Soundness (issue #547, fixed in PR #646)
    ///
    /// The key property: **no `&mut [Value]` covering this allocation is live
    /// while any code that dereferences `regs_ptr` is executing.**
    ///
    /// This is guaranteed by the dispatch-loop API change in PR #646: every
    /// `run_bytecode*` function now accepts `RegSlice` (raw pointer + len)
    /// instead of `&mut [Value]`.  `RegSlice` carries no LLVM `noalias`
    /// annotation, so dereferencing `regs_ptr` in any helper while the
    /// dispatch loop is blocked inside `call_function_expanded` is not aliasing
    /// UB — both accesses go through raw pointers.
    ///
    /// The old invariant ("per-element `NonNull::as_ref()` limits the alias
    /// scope") was insufficient: even a single-element `&Value` / `&mut Value`
    /// formed while a `&mut [Value]` to the same allocation is live on the
    /// call stack violates Rust/LLVM `noalias` rules.  Removing `&mut [Value]`
    /// from the picture entirely (via `RegSlice`) is the correct fix.
    ///
    /// Access patterns:
    /// * **Reads** (`merge_frame_view_into_dict`, `Insn::LoadGlobal`): happen
    ///   while the frame is suspended inside `call_function_expanded`.  The
    ///   interpreter is single-threaded; no concurrent writes are possible.
    /// * **Writes** (`assign_name`/`StoreGlobal`, `Insn::DeleteGlobal`): only
    ///   ever target a **suspended** `Script` frame from a nested `Function`
    ///   or `Class` frame.
    ///
    /// `NonNull` (rather than `*mut`) makes the non-null invariant explicit
    /// and catches null-push bugs at the push site rather than at dereference.
    pub(crate) regs_ptr: std::ptr::NonNull<Value>,
    pub(crate) regs_len: usize,
    pub(crate) local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
    /// Names declared `nonlocal` in this function frame (absent for
    /// `Script` frames which have no enclosing function scope).  Used
    /// by `snapshot_current_locals` (issue #486) to include nonlocal
    /// bindings in `locals()` — these live in an enclosing env, not
    /// in the fastlocal register file.
    pub(crate) nonlocal_names: Option<Rc<std::collections::HashSet<String>>>,
    /// The active env reference for this frame.  For function frames,
    /// this is the function's own local env (the one pushed by
    /// `call_user_function_expanded` before `run_bytecode`).
    /// `snapshot_current_locals` uses it as the starting point for
    /// `find_enclosing_local_env_for_name` to resolve nonlocal names.
    /// `None` for `Script` frames (which use the module env directly).
    pub(crate) env: Option<EnvRef>,
    /// True when this function was compiled as a direct method inside a class
    /// body (`FnCode::is_class_method`).  Used by `resolve_zero_arg_super` to
    /// identify the frame whose register 0 (`self`/`cls`) should be used.
    /// Script and Class frames always have this `false`.
    pub(crate) is_class_method: bool,
}

/// Thin wrapper around `iter_values` matching pyrust-core's `IterValuesFn`
/// signature (`&Value -> Result<Vec<Value>>`).  Installed at interpreter
/// startup so `pyrust-builtins` iterator helpers can drain arbitrary
/// sources without depending on this crate.
fn iter_values_for_registry(value: &Value) -> Result<Vec<Value>> {
    iter_values(value.clone())
}

/// Thin wrapper around `helpers::compare_values` matching pyrust-core's
/// `CompareValuesFn` signature (`(&Value, &Value) -> Result<Ordering>`).
/// Installed at interpreter startup so `pyrust-builtins` sort helpers
/// (`list.sort`, `sort_with_precomputed_keys`) can route through the
/// same canonical comparison the `<` / `>` operators use — covering
/// BigInt, nested List, etc.  See issue #428.
fn compare_values_for_registry(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
    compare_values(a, b)
}

impl Default for Interpreter {
    fn default() -> Self {
        pyrust_builtins::install();
        pyrust_core::install_iter_values(iter_values_for_registry);
        pyrust_core::install_compare_values(compare_values_for_registry);
        let env = Environment::new(None);
        // Exception classes and NotImplemented are no longer inserted into the
        // module env at startup.  They are resolved lazily: `LoadGlobal` falls
        // through to `resolve_builtin` which calls `lookup_exc_class` / returns
        // `Value::not_implemented()`.  `ExcClasses` is also lazy — the class
        // hierarchy is built from `EXC_CLASS_CACHE` on the first raise.
        // Scripts that never raise or name-check exceptions pay zero class-build cost.
        Self {
            env,
            active_exception: None,
            handled_exc_stack: Vec::new(),
            script_dir: None,
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            env_pool: Vec::new(),
            fn_cache: HashMap::new(),
            call_arg_buf: Vec::new(),
            bound_method_pos_buf: Vec::new(),
            key_scratch: Vec::new(),
            class_store_order: Vec::new(),
            vm_frame_views: Vec::new(),
            eq_in_progress: Vec::new(),
            exc_classes: ExcClasses::uninitialized(),
            module_globals_dict: Value::dict(IndexMap::new()),
            globals_accessed: false,
            global_env_version: Cell::new(0),
        }
    }
}

/// A raw-pointer view of a VM frame's register file, used by the dispatch loop
/// in place of `&mut [Value]` to eliminate the LLVM `noalias` UB described in
/// issue #547 and the Copilot review on PR #646.
///
/// # Why not `&mut [Value]`?
///
/// `&mut [Value]` carries LLVM's `noalias` attribute: the compiler may assume
/// no other pointer aliases the same allocation while that reference is live.
/// The invariant is violated when `VmFrameView` stores a `NonNull<Value>` into
/// the same allocation and the `globals()`/`locals()` helpers or
/// `StoreGlobal`/`DeleteGlobal` dereference it while the dispatch loop holds
/// `&mut [Value]` on the call stack.  `RegSlice` (raw pointer + len) replaces
/// `&mut [Value]` in every dispatch-loop signature; raw pointers carry no
/// aliasing semantics in Rust's type system, so the concurrent dereferences
/// through `VmFrameView::regs_ptr` are sound.
///
/// # Invariants the caller must uphold
///
/// - `ptr` is non-null, correctly aligned, and valid for `len` consecutive
///   `Value` reads/writes for the duration of the dispatch-loop call.
/// - No `&mut [Value]` covering the same allocation is held live (on the call
///   stack or otherwise) while the dispatch loop runs.
/// - No two mutable references to the same slot obtained through `IndexMut`
///   are alive simultaneously (trivially satisfied by normal borrow rules on
///   the returned `&mut Value`).
///
/// These invariants hold at every call site:
/// - `calls.rs`: the `RegsBuf` local is alive, not borrowed as `&mut [Value]`,
///   and the dispatch loop owns the only path to the allocation.
/// - `program.rs`: same pattern; `RegsBuf` is only accessed through `regs[i]`
///   after `run_bytecode` returns (no concurrent access).
/// - `vm.rs` generator resumes: `GeneratorFrame::regs` is on the heap
///   (stable address across yields) and is not borrowed as `&mut [Value]`.
pub(crate) struct RegSlice {
    ptr: *mut Value,
    len: usize,
}

// SAFETY: The interpreter is single-threaded (uses `Rc`, `RefCell`, and
// `thread_local!` throughout).  `RegSlice` is never moved across threads.
// `*mut Value` is not `Send`/`Sync` by default; we do not implement those
// traits, keeping `RegSlice` confined to its originating thread.

impl RegSlice {
    /// Construct a `RegSlice` from a raw pointer and length.
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null, aligned, and valid for `len` `Value` slots for
    /// at least as long as the returned `RegSlice` is in use.  No `&mut [Value]`
    /// covering the same allocation may be alive concurrently.
    #[inline]
    pub(crate) unsafe fn from_raw(ptr: *mut Value, len: usize) -> Self {
        debug_assert!(!ptr.is_null());
        RegSlice { ptr, len }
    }

    /// Number of register slots.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Iterate mutably over every slot.  Yields `&mut Value` one at a time;
    /// no two yielded references overlap, satisfying Rust's aliasing rules.
    #[inline]
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Value> {
        // SAFETY: ptr..ptr+len is valid and aligned (struct invariant);
        // from_raw_parts_mut produces the canonical slice for that range.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len).iter_mut() }
    }
}

/// Read-only view via `Deref` — reconstructs `&[Value]` from the raw pointer.
/// `&[Value]` (shared reference) does NOT carry LLVM `noalias`, so this is safe
/// to produce even while `VmFrameView` holds another pointer to the allocation.
impl std::ops::Deref for RegSlice {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        // SAFETY: ptr/len satisfy the struct invariant; &[Value] imposes no
        // exclusivity guarantee, so forming it alongside VmFrameView's raw
        // pointer is not aliasing UB.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

/// Indexed read: `regs[i]` — shared reference to slot `i`.
impl std::ops::Index<usize> for RegSlice {
    type Output = Value;
    #[inline]
    fn index(&self, i: usize) -> &Value {
        assert!(i < self.len, "RegSlice: index {i} >= len {}", self.len);
        // SAFETY: i < len, ptr is valid.
        unsafe { &*self.ptr.add(i) }
    }
}

/// Indexed write: `regs[i] = val` — exclusive reference to slot `i`.
/// The returned `&mut Value` is scoped to the surrounding expression and does
/// not alias any simultaneously-held reference.
impl std::ops::IndexMut<usize> for RegSlice {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut Value {
        assert!(i < self.len, "RegSlice: index_mut {i} >= len {}", self.len);
        // SAFETY: i < len, ptr is valid; the &mut lifetime is bounded by &mut self,
        // preventing it from outliving the RegSlice or aliasing another &mut.
        unsafe { &mut *self.ptr.add(i) }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

pub(crate) mod builtin_args;

include!("interpreter/tests.rs");
