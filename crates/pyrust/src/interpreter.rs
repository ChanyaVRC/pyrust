use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use indexmap::IndexMap;
use smallvec::smallvec;

use crate::ast::{AssignTarget, BinaryOp, Expr, Stmt, UnaryOp};
use crate::bytecode::{FnCode, GlobalCacheEntry};
use crate::error::{PyError, Result};
use crate::value::{
    EnvRef, Environment, GeneratorCell, GeneratorKind, InstanceAttrs, ModuleMutationState,
    PyBigInt, PyBigIntSign, PyClass, PyDict, PyInstance, PyKey, PyModule, PyPow, PySet,
    PyToPrimitive, PyZero, StrKey, UserFunction, UserFunctionKind, UserFunctionParam, Value,
    ValueKind, i64_range_native_cursor_safe, intern_string, intern_string_value, range_len,
};
use pyrust_core::CollectionMutationState;

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;
type MemoKey = (u64, smallvec::SmallVec<[i64; 3]>);

const DEFAULT_RECURSION_LIMIT: usize = 1000;
const ENV_POOL_MAX: usize = 64;
const MODULE_CLASS_CACHE_SLOT_COUNT: usize = 1;

/// Typed index into an Interpreter-owned module/class resolution cache.
///
/// Built-in module owners allocate a stable slot and provide the module and
/// attribute names only on a cache miss. The generic import layer validates
/// the `sys.modules` binding, its dictionary contents, and module-namespace
/// mutation generations, so the fast path never guesses from a Python-visible
/// class or module name.
#[derive(Clone, Copy)]
pub(crate) struct ModuleClassCacheSlot(usize);

impl ModuleClassCacheSlot {
    pub(crate) const fn new(index: usize) -> Self {
        Self(index)
    }
}

struct CachedModuleClass {
    /// Mutation generation of the canonical `sys.__dict__` which owns the
    /// Python-visible `modules` binding. Attribute assignment, deletion, and
    /// direct namespace aliases must invalidate an entry even when the old
    /// registry dict itself was not mutated.
    registry_owner_state: CollectionMutationState,
    registry_owner_version: u64,
    registry_state: CollectionMutationState,
    registry_version: u64,
    module_state: ModuleMutationState,
    module_version: u64,
    /// Cache metadata must not become an additional Python-visible owner of a
    /// class from a replaceable module generation.
    class: Weak<RefCell<PyClass>>,
}

struct ModuleClassCache {
    entries: [Option<CachedModuleClass>; MODULE_CLASS_CACHE_SLOT_COUNT],
}

/// Per-active-class namespace and PEP 695 type-alias bookkeeping.
///
/// `slot_names` makes `RecordClassStore`/`RecordClassDel` an O(1) name lookup,
/// and `scopes` contains only weak references so an abandoned alias cannot be
/// kept alive by the interpreter while the class body runs.
pub(crate) struct ActiveClassAnnotationScopes {
    slot_names: Vec<Option<String>>,
    scopes: Vec<Weak<RefCell<Environment>>>,
    /// The seed is normally second in class-namespace order, but deleting it
    /// before first introspection means a later rebind must move to the tail.
    qualname_slot: Option<crate::bytecode::Reg>,
    qualname_was_deleted: bool,
    /// Lazily materialized class-frame namespace.  Keeping the Python dict in
    /// the class-state stack makes every inner/outer frame view return the same
    /// mapping without allocating anything for classes that are never
    /// introspected.
    live_namespace: RefCell<Option<LiveClassNamespace>>,
}

pub(crate) struct LiveClassNamespace {
    value: Value,
    slot_names: Vec<Option<String>>,
}

/// Non-owning fast path for the Python-visible import registry.
///
/// Import lookup must observe both replacement of the canonical `sys` module
/// and every mutation route to its live namespace, but resolving the module
/// attribute on every cached import adds two unrelated hash lookups. Keep only
/// weak identities and the namespace generation so this cache cannot extend
/// the lifetime of a replaced dictionary or anything it owns.
struct CachedImportModuleRegistry {
    system_module: Weak<RefCell<PyModule>>,
    registry_owner_state: CollectionMutationState,
    registry_owner_version: u64,
    registry: pyrust_core::WeakValueCache,
}

impl Default for ModuleClassCache {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
        }
    }
}

/// Pre-resolved table of built-in exception classes.
///
/// Initialised lazily on the first call to `get` — the exception hierarchy is
/// built via the thread-local `EXC_CLASS_CACHE` only when actually needed.
/// Scripts that never raise or catch exceptions pay zero startup cost.
/// Lazily-built map from exception-class name to its resolved [`PyClass`].
/// Insertion ordered so publishing it into `builtins` yields a stable
/// `vars(builtins)` (issue #2918).
type ExcClassMap = indexmap::IndexMap<&'static str, Rc<RefCell<PyClass>>>;

pub(crate) struct ExcClasses(RefCell<Option<ExcClassMap>>);

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
    pub(crate) active_exception: Option<Value>,
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
    /// Parallel save-stack for `active_exception` across nested `except`
    /// blocks.  One entry is pushed (recording the *previous*
    /// `active_exception`) each time `handle_vm_error` dispatches an
    /// exception to a handler, and one entry is popped on `EndExcept` (normal
    /// exit) or `RaiseReRaise` (re-raise).  The popped value is restored to
    /// `active_exception`, so the outer handler's exception is visible again
    /// after an inner handler exits.
    ///
    /// This mirrors CPython's per-frame `_PyErr_StackItem` chain.  Without
    /// it, the dedup-pop inside `handle_vm_error` (which removes the outer
    /// exception from `handled_exc_stack` when a new inner exception is
    /// dispatched) means `EndExcept`'s `handled_exc_stack.last()` lookup
    /// returns `None` instead of the outer exception.
    exc_saved_active: Vec<Option<Value>>,
    script_dir: Option<PathBuf>,
    /// Path to the top-level script being executed, used to populate the
    /// `filename` field of `FrameInfo` when formatting tracebacks.
    /// `None` in REPL mode (no persistent filename).
    /// Stored as `Arc<str>` so that each call frame can clone the filename
    /// with a reference-count bump rather than a heap allocation.
    pub(crate) script_filename: Option<Arc<str>>,
    /// The process-level argument vector visible to a script as `sys.argv`.
    /// Empty when the interpreter has no script invocation context (for
    /// example, a library caller executing an in-memory program).
    script_argv: Vec<String>,
    module_cache: ModuleCache,
    /// Bootstrap `sys.modules` dictionary for this interpreter.
    ///
    /// This is authoritative only until the canonical `sys` module exists.
    /// Thereafter import operations resolve its current Python-visible
    /// `modules` attribute, so rebinding or deleting that attribute is observed.
    /// Root interpreters own independent bootstrap registries; an imported-file
    /// child shares its parent's backing for the pre-`sys` case.
    bootstrap_module_registry: Value,
    /// Non-owning cache of the active Python-visible import registry.
    ///
    /// This is separate from `module_class_cache`: every import consumes it,
    /// while class resolution additionally guards registry contents and module
    /// namespace generations.
    import_module_registry_cache: RefCell<Option<CachedImportModuleRegistry>>,
    /// Lazily allocated cache for owner-requested module class lookups.
    ///
    /// Most interpreters never use this. `typing.Generic[...]` currently owns
    /// the first slot because its process-canonical receiver cannot itself
    /// identify the active Interpreter's reload generation.
    module_class_cache: Option<Box<ModuleClassCache>>,
    /// Canonical filter and recording-context state for the `warnings` module.
    ///
    /// The concrete policy data remains private to the built-in module. Root
    /// interpreters own independent handles; filesystem-import children clone
    /// the handle because they are an implementation detail of the same
    /// Python interpreter and share `sys.modules` with their parent.
    pub(crate) warnings_state: Rc<crate::builtin_modules::warnings::WarningsState>,
    /// Decimal integer string-conversion limit configured through
    /// `sys.set_int_max_str_digits` (0 means unlimited).
    ///
    /// The core value layer consumes this through a scoped TLS adapter only
    /// while this interpreter is executing. Keeping the authoritative value
    /// here prevents independent roots on one host thread from leaking policy.
    int_max_str_digits: usize,
    /// Maximum Python call depth for this interpreter.
    ///
    /// The active depth remains thread-local because nested Interpreter calls
    /// share one host stack. The configurable limit is interpreter-owned so
    /// independent roots on the same thread cannot change one another's
    /// `sys.setrecursionlimit` setting.
    recursion_limit: usize,
    env_pool: Vec<EnvRef>,
    /// Automatic memoization of pure scalar functions (#2234): caches the result
    /// of a `CallMemo` (statically-pure callee) keyed by `(fn_id, integer
    /// args)`, when the result is a value-identity scalar (int/bool/None) so the
    /// cache is observably transparent.  `memo_stats` drives an adaptive gate:
    /// a function whose hit-rate stays low after a warmup is disabled, so the
    /// common varying-argument case pays nothing (the reason #1987 removed the
    /// previous always-on cache).
    memo_cache: std::collections::HashMap<MemoKey, Value>,
    memo_stats: std::collections::HashMap<u64, (u32, u32, bool)>,
    /// Memo keys whose cache miss is currently executing.
    ///
    /// A direct recursive call with the same key cannot produce a cache entry
    /// until its caller returns. Declining that nested miss lets the VM's
    /// explicit frame trampoline enforce the Python recursion limit without
    /// growing the native Rust stack.
    memo_in_flight: std::collections::HashSet<MemoKey>,
    /// Reusable argument buffer for VM Call instructions — avoids a per-call
    /// heap allocation in the common (non-recursive) case.
    call_arg_buf: Vec<ExpandedCallArg>,
    /// Reusable argument buffer for `invoke_class_method`'s `BuiltinFunction`
    /// path — avoids a per-invocation heap allocation on the hot dunder
    /// dispatch path (`__add__`, `__iter__`, `__next__`, `__len__`, …).
    /// Pattern: `std::mem::take`, clear, fill `self + args`, call dispatch,
    /// then restore so subsequent invocations reuse the grown capacity.
    /// On recursive entry the field is empty (already taken), so a fresh
    /// SmallVec is allocated only for the nested frame.
    pub(crate) invoke_arg_buf: ExpandedArgBuf,
    /// Reusable positional-args buffer for the builtin bound-method dispatch
    /// path — avoids a per-call heap allocation on the hot path (issue #276).
    /// Pattern: `std::mem::take`, clear, fill, use (borrow or drain), then
    /// restore so subsequent calls reuse the grown capacity.
    bound_method_pos_buf: Vec<Value>,
    /// Stack of class-namespace store-order lists, pushed by `MakeClass` before
    /// running a class body and popped after.  Each entry tracks the slot numbers
    /// for class-body locals in the **order stores actually executed** at
    /// runtime — CPython's `__dict__` ordering rule.  A stack (not a single Vec)
    /// supports nested `class A: class B: ...` bodies cleanly.
    pub(crate) class_store_order: Vec<Vec<u32>>,
    /// Per-active-class state parallel to `class_store_order`: PEP 695 alias
    /// evaluators plus the lazily materialized live class-frame namespace.
    /// Class-store opcodes update only the state for their exact body.
    pub(crate) class_annotation_scopes: Vec<ActiveClassAnnotationScopes>,
    /// Stack of active VM frame views (issue #389).  Pushed before each
    /// `run_bytecode` invocation and popped immediately afterwards by:
    ///   * `try_exec_vm_script_with_index` (kind = `Script`), and
    ///   * `call_user_function_expanded`'s register-VM tier (kind =
    ///     `Function`), and
    ///   * `run_class_body` (kind = `Class`).
    ///
    /// Each entry holds a raw pointer to the active frame's register
    /// file plus the name -> slot mapping the compiler emitted.
    /// Built-ins like `globals()` / `locals()` consult this stack to
    /// surface names that would otherwise live only in registers:
    ///   * `globals()` walks down to the BOTTOM-most `Script` entry to
    ///     find the module's fastlocals (e.g. top-level `x = 5`).
    ///   * `locals()` reads the TOP entry — the innermost function/class frame,
    ///     or the script frame at module scope.
    ///
    /// Safety: each raw pointer is only dereferenced while the frame
    /// is still on the VM stack, i.e. inside the same VM entry that
    /// pushed it.  The push/pop invariant is maintained by:
    ///   * `program_execution::try_exec_vm_script_with_index` (`Script`)
    ///   * `calls::call_user_function_expanded` — both the simple
    ///     and variadic user-function paths (`Function`)
    ///   * `execution::resume_generator_with_exc` — each generator resume
    ///     (`Function`; the regs pointer comes from the heap-allocated
    ///     `GeneratorFrame::regs`, stable across yields)
    ///
    /// Class bodies (`Insn::MakeClass`) publish a `FrameKind::Class` view
    /// so that `locals()` inside a class body returns the partially-built
    /// class attrs dict (issue #487).
    pub(crate) vm_frame_views: Vec<VmFrameView>,
    /// Lazily allocated frame-object caches parallel to `vm_frame_views` for
    /// non-generator activations that have actually been introspected. The
    /// vector ends at its highest materialized slot; generator activations own
    /// their persistent cache in `GeneratorFrame` instead and never add a slot.
    /// The extra box intentionally keeps this cold field to one word.
    #[allow(clippy::box_collection)]
    pub(crate) vm_frame_caches: Option<Box<Vec<Option<Box<VmFrameCache>>>>>,
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
    /// Tracks the nesting depth of `PushExcContext` instructions currently
    /// in progress.  Incremented by `PushExcContext`, decremented by
    /// `PopExcContext`.  While non-zero, `handle_vm_error` must NOT perform
    /// its "active-exception duplicate pop" because the top of
    /// `handled_exc_stack` was placed there by `PushExcContext` (the
    /// to-be-raised exception), not by the normal handler-dispatch path.
    /// Popping it prematurely would lose the context entry and cause any
    /// exception raised inside the finally block to get a `None` context
    /// instead of the correct value.
    pub(crate) push_exc_ctx_depth: u32,
    /// Set by a bare `raise` / implicit re-raise (`RaiseReRaise`) and consumed
    /// by the next catch site (issue #2367).  When set, the catch rebuilds the
    /// traceback from the captured unwind frames rather than *prepending* onto
    /// the exception's carried chain — bare re-raise keeps the original chain's
    /// head line, which pyrust's prepend path would otherwise double-count.
    /// An explicit `raise e` / `raise e.with_traceback(...)` clears it, so the
    /// prepend path (issue #2367's actual scope) runs for those.
    pub(crate) reraise_is_bare: bool,
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
    /// Script frames retain their root env so namespace-sensitive helpers can
    /// distinguish an imported function's globals from the caller's script
    /// registers.
    pub(crate) env: Option<EnvRef>,
    /// True when this function was compiled as a direct method inside a class
    /// body (`FnCode::is_class_method`).  Used by `resolve_zero_arg_super` to
    /// identify the frame whose register 0 (`self`/`cls`) should be used.
    /// Script and Class frames always have this `false`.
    pub(crate) is_class_method: bool,
    /// The `UserFunction` this frame is executing, for `Function` frames.
    /// `None` for `Script` and `Class` frames.  Stored as a strong `Rc`
    /// (a refcount bump at push time — the function is already alive for the
    /// duration of its own call) so that frame-introspection objects
    /// (`sys._getframe`, traceback `tb_frame`; issues #2170/#2171) can recover
    /// the frame's `co_name` / code object without threading the name through
    /// the VM dispatch signatures.
    pub(crate) function: Option<Rc<UserFunction>>,
    /// For generator frames (which carry no `UserFunction`, so `function` is
    /// `None`), a raw pointer to the active `GeneratorFrame` from which the
    /// generator's `(funcname, filename)` is recovered *lazily* on the cold
    /// traceback path (issue #2471).  Used so a traceback built when an
    /// exception is *caught inside* the generator body (e.g. via
    /// `generator.throw()`) attributes the catching frame to the generator
    /// rather than falling back to the `<module>` frame (issue #2445).  `None`
    /// for every non-generator frame.
    ///
    /// Storing one pointer rather than two eagerly-cloned `Arc<str>`s keeps
    /// `VmFrameView` narrow and the hot `vm_enter_gen_drive` setup free of the
    /// two per-resume atomic refcount bumps that PR #2469 introduced: the
    /// name/filename are read only when a traceback snapshot is actually built
    /// (always the cold path).
    ///
    /// # Soundness
    ///
    /// The `GeneratorFrame` is heap-stable for the lifetime of this view.  At
    /// both push sites it lives behind a stable address — the trampoline's
    /// `Box<GeneratorFrame>` (held in `GenDriveFrame::gframe`) or a
    /// `&mut GeneratorFrame` borrowed for the whole duration of
    /// `resume_generator_with_exc` — and the view is popped before that box /
    /// borrow can be used again. While the dispatch loop is running, cold-path
    /// consumers reconstruct shared references only; they never reconstruct a
    /// raw-derived mutable reference. The frame cache alone uses interior
    /// mutability, with each `RefCell` borrow confined to one cold helper call.
    pub(crate) gen_frame: Option<std::ptr::NonNull<GeneratorFrame>>,
}

pub(crate) struct VmFrameCache {
    pub(crate) object: pyrust_core::WeakValueCache,
    /// Weak handle to this activation's function-locals mapping. Retaining
    /// only `frame.f_locals` must make later frame lookups reuse and refresh
    /// that dict without making the activation cache a new owner itself.
    pub(crate) function_locals: Option<std::rc::Weak<RefCell<PyDict>>>,
    /// Compiler-owned function-local keys synchronized into the persistent
    /// `f_locals` dict. Mapping-only keys inserted through `f_locals` are not
    /// recorded here and therefore survive later fastlocals refreshes.
    pub(crate) function_local_names: Vec<PyKey>,
}

/// Thin wrapper around `iter_values` matching pyrust-core's `IterValuesFn`
/// signature (`&Value -> Result<Vec<Value>>`).  Installed at interpreter
/// startup so `pyrust-builtins` iterator helpers can drain arbitrary
/// sources without depending on this crate.
fn iter_values_for_registry(value: &Value) -> Result<Vec<Value>> {
    iter_values(value)
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
        pyrust_core::install_builtin_callable_presentation_provider(builtin_callable_presentation);
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
            exc_saved_active: Vec::new(),
            script_dir: None,
            script_filename: None,
            script_argv: Vec::new(),
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            bootstrap_module_registry: Value::dict(PyDict::default()),
            import_module_registry_cache: RefCell::new(None),
            module_class_cache: None,
            warnings_state: Rc::new(crate::builtin_modules::warnings::WarningsState::default()),
            int_max_str_digits: pyrust_core::INT_MAX_STR_DIGITS_DEFAULT,
            recursion_limit: DEFAULT_RECURSION_LIMIT,
            env_pool: Vec::new(),
            memo_cache: std::collections::HashMap::new(),
            memo_stats: std::collections::HashMap::new(),
            memo_in_flight: std::collections::HashSet::new(),
            call_arg_buf: Vec::new(),
            invoke_arg_buf: ExpandedArgBuf::new(),
            bound_method_pos_buf: Vec::new(),
            class_store_order: Vec::new(),
            class_annotation_scopes: Vec::new(),
            vm_frame_views: Vec::new(),
            vm_frame_caches: None,
            eq_in_progress: Vec::new(),
            exc_classes: ExcClasses::uninitialized(),
            push_exc_ctx_depth: 0,
            reraise_is_bare: false,
        }
    }
}

/// A raw-pointer view of a VM frame's register file, used by the dispatch loop
/// in place of `&mut [Value]` to eliminate the LLVM `noalias` UB described in
/// issues #547 and #648 and the Copilot review on PR #646.
///
/// # Why not `&mut [Value]`?
///
/// `&mut [Value]` carries LLVM's `noalias` attribute: the compiler may assume
/// no other pointer aliases the same allocation while that reference is live.
/// The invariant is violated when `VmFrameView` stores a `NonNull<Value>` into
/// the same allocation and the `globals()`/`locals()` helpers or
/// `StoreGlobal`/`DeleteGlobal` dereference it while the dispatch loop holds
/// `&mut [Value]` on the call stack.  `RegSlice` (raw pointer + len) replaces
/// `&mut [Value]` in every dispatch-loop signature. The raw pointer itself
/// carries no reference aliasing promise, so a `VmFrameView` may retain it
/// while the loop runs. References reconstructed from either pointer still
/// obey Rust's ordinary rules: a shared slot reference must end before that
/// slot is mutated, and an exclusive slot reference must not overlap any other
/// reference to the slot.
///
/// # Invariants the caller must uphold
///
/// - `ptr` is non-null, correctly aligned, and valid for `len` consecutive
///   `Value` reads/writes for the duration of the dispatch-loop call.
/// - No `&mut [Value]` covering the same allocation is held live (on the call
///   stack or otherwise) while the dispatch loop runs.
/// - A shared reference reconstructed by `Deref` or `Index` is not held across
///   a re-entrant operation that can mutate the same register allocation
///   through a `VmFrameView`.
/// - No two mutable references to the same slot obtained through `IndexMut`
///   are alive simultaneously (trivially satisfied by normal borrow rules on
///   the returned `&mut Value`).
///
/// These invariants hold at every call site:
/// - `calls`: the `RegsBuf` local is alive, not borrowed as `&mut [Value]`,
///   and the dispatch loop owns the only path to the allocation.
/// - `program_execution`: same pattern; `RegsBuf` is only accessed through
///   `regs[i]`
///   after `run_bytecode` returns (no concurrent access).
/// - `execution` generator resumes: `GeneratorFrame::regs` is on the heap
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

    #[inline]
    pub(crate) fn non_null_ptr(&self) -> std::ptr::NonNull<Value> {
        // SAFETY: non-nullness is a construction invariant of `RegSlice`.
        unsafe { std::ptr::NonNull::new_unchecked(self.ptr) }
    }
}

/// Read-only view via `Deref` — reconstructs `&[Value]` from the raw pointer.
///
/// A raw `VmFrameView` pointer may coexist with this slice, but it must not be
/// used to mutate any covered slot until the returned shared reference is no
/// longer live.
impl std::ops::Deref for RegSlice {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        // SAFETY: ptr/len satisfy the struct invariant. Callers must uphold the
        // reference-lifetime rule documented above before mutating through an
        // aliasing raw frame-view pointer.
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

// ─── Code object (result of compile()) ───────────────────────────────────────
//
// pyrust represents `compile()` results as `BuiltinObject` values whose state
// holds a `CodeState`.  `exec()` and `eval()` detect these by the type name
// `"code"` and extract the precompiled `FnCode` and mode.  This avoids adding
// a new `ValueKind` variant to `pyrust-core`.

/// The two modes in which compiled code can be run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodeMode {
    /// Run as a sequence of statements; return `None`.
    Exec,
    /// Run as a single expression; return the expression's value.
    Eval,
}

/// State held by a code object `Value`.
pub(crate) struct CodeState {
    pub(crate) code: Rc<FnCode>,
    pub(crate) mode: CodeMode,
    /// Local-index table used when compiling/running the code.
    pub(crate) local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
}

struct CodeObjectOps;

static CODE_OBJECT_OPS: CodeObjectOps = CodeObjectOps;

impl pyrust_core::BuiltinTypeOps for CodeObjectOps {
    fn type_name(&self) -> &'static str {
        "code"
    }

    fn repr(&self, _state: &pyrust_core::BuiltinState) -> String {
        "<code object>".to_string()
    }

    fn truthy(&self, _state: &pyrust_core::BuiltinState) -> bool {
        true
    }
}

/// Construct a `Value` wrapping a compiled code object.
pub(crate) fn value_code_object(
    code: Rc<FnCode>,
    mode: CodeMode,
    local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
) -> Value {
    use std::any::Any;
    let state: Box<dyn Any> = Box::new(CodeState {
        code,
        mode,
        local_index,
    });
    Value::builtin_object(&CODE_OBJECT_OPS, state)
}

/// If `value` is a code object, call `f` with its `CodeState` and return the result.
pub(crate) fn with_code_state<R>(value: &Value, f: impl FnOnce(&CodeState) -> R) -> Option<R> {
    if let ValueKind::BuiltinObject { ops, state } = value.kind()
        && pyrust_core::builtin_ops_is::<CodeObjectOps>(ops)
    {
        let borrow = state.borrow();
        let cs = borrow.downcast_ref::<CodeState>()?;
        return Some(f(cs));
    }
    None
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

pub(crate) mod builtin_args;

include!("interpreter/tests.rs");
