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
    ValueKind, i64_range_native_cursor_safe, intern_string, range_len,
};
use pyrust_core::CollectionMutationState;

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;
type MemoKey = (u64, smallvec::SmallVec<[i64; 3]>);
type ComparisonPair = (i64, i64);
type ComparisonContext = (usize, u64);
type ComparisonFrame = (i64, i64, usize, u64);
type ActiveComparisonContexts = HashMap<ComparisonPair, smallvec::SmallVec<[ComparisonContext; 1]>>;

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
    /// Immutable class body code-object identity (`co_name`, `co_filename`)
    /// surfaced by `sys._getframe()` and class-body tracebacks (issue #3029).
    pub(crate) class_name: std::sync::Arc<str>,
    pub(crate) filename: std::sync::Arc<str>,
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
type ExcClassMap = indexmap::IndexMap<&'static str, Rc<RefCell<PyClass>>, pyrust_core::PyHasher>;

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

/// One registered `atexit` callback entry: callable plus its bound positional
/// and keyword arguments (issue #3209).
pub(crate) struct AtexitCallback {
    pub(crate) func: Value,
    pub(crate) args: Vec<ExpandedCallArg>,
}

/// Per-interpreter runtime state for the `gc` module (issue #3246).
pub(crate) struct GcRuntimeState {
    pub(crate) enabled: bool,
    pub(crate) thresholds: (i64, i64, i64),
    pub(crate) counts: (i64, i64, i64),
    pub(crate) debug: i64,
    pub(crate) stats: [(i64, i64, i64); 3],
    pub(crate) freeze_count: i64,
    pub(crate) garbage: Value,
    pub(crate) callbacks: Value,
}

impl Default for GcRuntimeState {
    fn default() -> Self {
        Self {
            enabled: true,
            thresholds: (700, 10, 10),
            counts: (0, 0, 0),
            debug: 0,
            stats: [(0, 0, 0); 3],
            freeze_count: 0,
            garbage: Value::list(Vec::new()),
            callbacks: Value::list(Vec::new()),
        }
    }
}

/// Single traced allocation entry recorded by `_tracemalloc` (issue #3246).
#[derive(Clone, Debug)]
pub(crate) struct TracemallocRecord {
    pub(crate) domain: u64,
    pub(crate) size: usize,
    pub(crate) frames: Vec<(String, i64)>,
    pub(crate) total_nframe: usize,
}

/// Per-interpreter runtime state for `_tracemalloc` / `tracemalloc` (issue #3246).
pub(crate) struct TracemallocRuntimeState {
    pub(crate) tracing: bool,
    pub(crate) traceback_limit: usize,
    pub(crate) current_memory: usize,
    pub(crate) peak_memory: usize,
    pub(crate) tracked_objects: indexmap::IndexMap<i64, TracemallocRecord>,
    pub(crate) ignored_objects: std::collections::HashSet<i64>,
}

impl Default for TracemallocRuntimeState {
    fn default() -> Self {
        Self {
            tracing: false,
            traceback_limit: 1,
            current_memory: 0,
            peak_memory: 0,
            tracked_objects: indexmap::IndexMap::new(),
            ignored_objects: std::collections::HashSet::new(),
        }
    }
}

/// Per-interpreter runtime state for `faulthandler` (issue #3246).
pub(crate) struct FaulthandlerRuntimeState {
    pub(crate) enabled: bool,
    pub(crate) all_threads: bool,
    pub(crate) enabled_file: Option<Value>,
    pub(crate) later_cancel: Option<std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>>,
    #[cfg(unix)]
    pub(crate) registered_signals: std::collections::HashMap<i32, (bool, bool)>,
}

impl Default for FaulthandlerRuntimeState {
    fn default() -> Self {
        Self {
            enabled: false,
            all_threads: true,
            enabled_file: None,
            later_cancel: None,
            #[cfg(unix)]
            registered_signals: std::collections::HashMap::new(),
        }
    }
}

impl Drop for FaulthandlerRuntimeState {
    fn drop(&mut self) {
        if let Some(pair) = self.later_cancel.take() {
            let (lock, cvar) = &*pair;
            if let Ok(mut cancelled) = lock.lock() {
                *cancelled = true;
                cvar.notify_all();
            }
        }
    }
}

/// Per-interpreter runtime state for `sys.monitoring` (PEP 669, issue #3246).
pub(crate) struct MonitoringRuntimeState {
    pub(crate) tools: [Option<String>; 6],
    pub(crate) global_events: [i64; 6],
    pub(crate) local_events: [std::collections::HashMap<i64, i64>; 6],
    pub(crate) callbacks: [std::collections::HashMap<i64, Value>; 6],
}

impl Default for MonitoringRuntimeState {
    fn default() -> Self {
        Self {
            tools: std::array::from_fn(|_| None),
            global_events: [0; 6],
            local_events: std::array::from_fn(|_| std::collections::HashMap::new()),
            callbacks: std::array::from_fn(|_| std::collections::HashMap::new()),
        }
    }
}

/// Cold, rarely-used interpreter metadata, caches, and diagnostic/guard tables
/// extracted behind a single `Box<InterpreterColdState>` (issue #3162).
///
/// Keeping these out of line shrinks `size_of::<Interpreter>()` from ~1008 B
/// to ~272 B so hot VM state (`env`, `active_exception`, `vm_frame_views`,
/// `tramp_arena`, `tramp_stack`, `gen_drive_stack`, call buffers) stays
/// densely packed in the first few cache lines.
pub(crate) struct InterpreterColdState {
    script_dir: Option<PathBuf>,
    /// Path to the top-level script being executed, used to populate the
    /// `filename` field of `FrameInfo` when formatting tracebacks.
    /// `None` in REPL mode (no persistent filename).
    /// Stored as `Arc<str>` so that each call frame can clone the filename
    /// with a reference-count bump rather than a heap allocation.
    pub(crate) script_filename: Option<Arc<str>>,
    /// Whether `seed_module_dunders` should expose `__file__` from
    /// `script_filename`. `false` for `-c` / `-` invocations where code
    /// objects carry `<string>` / `<stdin>` for tracebacks without defining a
    /// global `__file__`.
    script_define_file_dunder: bool,
    /// Optional `__package__` value for top-level `-m` module execution
    /// (`""` for a top-level `.py` module, `"pkg"` for `pkg.sub` or
    /// `pkg/__main__.py`). `None` seeds `__package__ = None`.
    script_package: Option<String>,
    /// The process-level argument vector visible to a script as `sys.argv`.
    /// Empty when the interpreter has no script invocation context (for
    /// example, a library caller executing an in-memory program).
    script_argv: Vec<String>,
    /// Guards `initialize_script_argv` from overwriting `sys.argv` once it has
    /// already been populated for the current interpreter invocation.
    script_argv_initialized: bool,
    pub(crate) module_cache: ModuleCache,
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
    /// Reusable argument buffer for `invoke_class_method`'s `BuiltinFunction`
    /// path — avoids a per-invocation heap allocation on the dunder
    /// dispatch path (`__add__`, `__iter__`, `__next__`, `__len__`, …).
    /// Pattern: `std::mem::take`, clear, fill `self + args`, call dispatch,
    /// then restore so subsequent invocations reuse the grown capacity.
    /// On recursive entry the field is empty (already taken), so a fresh
    /// SmallVec is allocated only for the nested frame.
    pub(crate) invoke_arg_buf: ExpandedArgBuf,
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
    /// Legacy recursion stack for aggregate set/view comparisons. Direct
    /// list, dict, and deque comparisons use the exception-capable pair stack
    /// below so a structural back-edge raises `RecursionError`.
    pub(crate) eq_in_progress: Vec<(i64, i64)>,
    /// Ordered container pairs active under CPython's exception-capable rich
    /// comparison policy. Call depth plus a comparison-dispatch epoch lets a
    /// user callback make finite progress without hiding a pure structural
    /// cycle.
    pub(crate) comparison_pair_stack: Vec<ComparisonFrame>,
    pub(crate) comparison_active_contexts: ActiveComparisonContexts,
    pub(crate) comparison_dispatch_epoch: u64,
    /// Number of retained native container-comparison frames.
    pub(crate) comparison_recursion_cost: usize,
    /// Structural-only root cost not charged to Python callback entry.
    pub(crate) comparison_structural_cost_bias: usize,
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
    /// Stack of `(handled_exc_stack_len, exc_saved_active_len)` marks recorded
    /// by `PushExcContext` and restored by `PopExcContext` (used for inlined
    /// `finally` blocks and PEP 654 `except*` handler scopes).  While the
    /// active exception sits at or below `saved_handled_len + 1`,
    /// `handle_vm_error` must NOT dedup-pop the pushed context entry.
    pub(crate) push_exc_ctx_marks: Vec<(usize, usize)>,
    /// When an inlined comprehension (`FnCode::is_inlined_comp`, PEP 709) exits
    /// with an error via a non-trampolined call or coroutine resume, it skips
    /// recording a `<listcomp>`/`<setcomp>`/`<dictcomp>` traceback frame and
    /// stashes its inner PEP 657 caret anchor here so the enclosing frame's
    /// `vm_try!` adopts that anchor instead of clobbering it (#2993).
    pub(crate) inlined_comp_escaped_span: Option<Option<(u32, u32, u32, u32)>>,
    /// Side-stack of `(caller_regs_ptr, line)` for non-trampolined calls so
    /// cold frame inspection (`warnings.warn(stacklevel=...)`) can recover the
    /// exact caller line without changing `VmFrameView` or `Interpreter` size.
    pub(crate) call_site_lines: Vec<(*mut Value, u32)>,
    /// Shared per-interpreter `atexit` callback registry (issue #3209).
    /// Cloned by `Rc` into filesystem-import child interpreters so callbacks
    /// registered in imported modules execute on top-level interpreter shutdown.
    pub(crate) atexit_state: Rc<RefCell<Vec<Option<AtexitCallback>>>>,
    /// Shared per-interpreter `gc` state (issue #3246).
    pub(crate) gc_state: Rc<RefCell<GcRuntimeState>>,
    /// Shared per-interpreter `_tracemalloc` / `tracemalloc` state (issue #3246).
    pub(crate) tracemalloc_state: Rc<RefCell<TracemallocRuntimeState>>,
    /// Shared per-interpreter `faulthandler` state (issue #3246).
    pub(crate) faulthandler_state: Rc<RefCell<FaulthandlerRuntimeState>>,
    /// Shared per-interpreter `sys.monitoring` state (PEP 669, issue #3246).
    pub(crate) monitoring_state: Rc<RefCell<MonitoringRuntimeState>>,
    /// Startup CLI flags and `PYTHON*` environment configuration (issue #3243).
    /// Shared via `Rc` with child filesystem-import interpreters.
    pub(crate) runtime_config: Rc<RuntimeConfig>,
    /// True when `initialize_cli_startup` manages `sys.path` ordering
    /// (`PYTHONPATH` -> `import site` -> prepend script/cwd unless `-P`/`-I`).
    pub(crate) cli_startup_managed: bool,
    /// True once `initialize_cli_startup` has run for this interpreter.
    pub(crate) cli_startup_initialized: bool,
}

/// Startup configuration populated from CPython 3.12 CLI flags and `PYTHON*`
/// environment variables (issue #3243).
///
/// Stored behind `Rc` inside [`InterpreterColdState`] so hot VM state and
/// `size_of::<Interpreter>()` are completely unaffected.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub debug: i64,
    pub inspect: i64,
    pub interactive: i64,
    pub optimize: i64,
    pub dont_write_bytecode: i64,
    pub no_user_site: i64,
    pub no_site: i64,
    pub ignore_environment: i64,
    pub verbose: i64,
    pub bytes_warning: i64,
    pub quiet: i64,
    pub hash_randomization: i64,
    pub isolated: i64,
    pub dev_mode: bool,
    pub utf8_mode: i64,
    pub warn_default_encoding: i64,
    pub safe_path: bool,
    pub int_max_str_digits: i64,
    pub unbuffered: bool,
    pub skip_first_line: bool,
    pub check_hash_based_pycs: String,
    pub warnoptions: Vec<String>,
    pub xoptions: Vec<(String, Option<String>)>,
    pub pycache_prefix: Option<String>,
    pub platlibdir: Option<String>,
    pub python_path: Vec<String>,
    pub prefix_override: Option<String>,
    pub exec_prefix_override: Option<String>,
    pub orig_argv: Vec<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            debug: 0,
            inspect: 0,
            interactive: 0,
            optimize: 0,
            dont_write_bytecode: 0,
            no_user_site: 0,
            no_site: 0,
            ignore_environment: 0,
            verbose: 0,
            bytes_warning: 0,
            quiet: 0,
            hash_randomization: 1,
            isolated: 0,
            dev_mode: false,
            utf8_mode: 0,
            warn_default_encoding: 0,
            safe_path: false,
            int_max_str_digits: pyrust_core::INT_MAX_STR_DIGITS_DEFAULT as i64,
            unbuffered: false,
            skip_first_line: false,
            check_hash_based_pycs: "default".to_string(),
            warnoptions: Vec::new(),
            xoptions: Vec::new(),
            pycache_prefix: None,
            platlibdir: None,
            python_path: Vec::new(),
            prefix_override: None,
            exec_prefix_override: None,
            orig_argv: Vec::new(),
        }
    }
}

impl Default for InterpreterColdState {
    fn default() -> Self {
        Self {
            script_dir: None,
            script_filename: None,
            script_define_file_dunder: true,
            script_package: None,
            script_argv: Vec::new(),
            script_argv_initialized: false,
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            bootstrap_module_registry: Value::dict(PyDict::default()),
            import_module_registry_cache: RefCell::new(None),
            module_class_cache: None,
            env_pool: Vec::new(),
            memo_cache: std::collections::HashMap::new(),
            memo_stats: std::collections::HashMap::new(),
            memo_in_flight: std::collections::HashSet::new(),
            invoke_arg_buf: ExpandedArgBuf::new(),
            class_store_order: Vec::new(),
            class_annotation_scopes: Vec::new(),
            eq_in_progress: Vec::new(),
            comparison_pair_stack: Vec::new(),
            comparison_active_contexts: HashMap::new(),
            comparison_dispatch_epoch: 0,
            comparison_recursion_cost: 0,
            comparison_structural_cost_bias: 0,
            exc_classes: ExcClasses::uninitialized(),
            push_exc_ctx_marks: Vec::new(),
            inlined_comp_escaped_span: None,
            call_site_lines: Vec::new(),
            atexit_state: Rc::new(RefCell::new(Vec::new())),
            gc_state: Rc::new(RefCell::new(GcRuntimeState::default())),
            tracemalloc_state: Rc::new(RefCell::new(TracemallocRuntimeState::default())),
            faulthandler_state: Rc::new(RefCell::new(FaulthandlerRuntimeState::default())),
            monitoring_state: Rc::new(RefCell::new(MonitoringRuntimeState::default())),
            runtime_config: Rc::new(RuntimeConfig::default()),
            cli_startup_managed: false,
            cli_startup_initialized: false,
        }
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
    /// Reusable register arena for trampolined Python→Python calls (#2234 / #3159).
    /// Lazily reserved once to a fixed capacity on the first trampolined call and
    /// never reallocated while live `RegSlice` / `VmFrameView` pointers borrow it.
    pub(crate) tramp_arena: Vec<Value>,
    /// Reusable stack of active trampolined call frames across nested VM entries (#3159).
    pub(crate) tramp_stack: Vec<TrampFrame>,
    /// Reusable stack of active inline-driven generator frames across nested VM entries (#3159).
    pub(crate) gen_drive_stack: Vec<GenDriveFrame>,
    /// Reusable argument buffer for VM Call instructions — avoids a per-call
    /// heap allocation in the common (non-recursive) case.
    call_arg_buf: Vec<ExpandedCallArg>,
    /// Reusable positional-args buffer for the builtin bound-method dispatch
    /// path — avoids a per-call heap allocation on the hot path (issue #276).
    /// Pattern: `std::mem::take`, clear, fill, use (borrow or drain), then
    /// restore so subsequent calls reuse the grown capacity.
    bound_method_pos_buf: Vec<Value>,
    /// Lazily allocated frame-object caches parallel to `vm_frame_views` for
    /// non-generator activations that have actually been introspected. The
    /// vector ends at its highest materialized slot; generator activations own
    /// their persistent cache in `GeneratorFrame` instead and never add a slot.
    /// The extra box intentionally keeps this cold field to one word.
    #[allow(clippy::box_collection)]
    pub(crate) vm_frame_caches: Option<Box<Vec<Option<Box<VmFrameCache>>>>>,
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
    /// Canonical filter and recording-context state for the `warnings` module.
    ///
    /// The concrete policy data remains private to the built-in module. Root
    /// interpreters own independent handles; filesystem-import children clone
    /// the handle because they are an implementation detail of the same
    /// Python interpreter and share `sys.modules` with their parent.
    pub(crate) warnings_state: Rc<crate::builtin_modules::warnings::WarningsState>,
    /// Shared per-interpreter `sys` runtime state (switch interval, coroutine
    /// origin tracking depth, profile/trace/asyncgen/audit hooks — issue
    /// #2970). Shared via `Rc` with child filesystem-import interpreters.
    pub(crate) sys_state: Rc<RefCell<SysRuntimeState>>,
    /// Boxed cold interpreter state: memo/comparison maps, `invoke_arg_buf`,
    /// class-only `env_pool`, script/import caches, and diagnostic tables (#3162).
    pub(crate) cold: Box<InterpreterColdState>,
    /// Set by a bare `raise` / implicit re-raise (`RaiseReRaise`) and consumed
    /// by the next catch site (issue #2367).  When set, the catch rebuilds the
    /// traceback from the captured unwind frames rather than *prepending* onto
    /// the exception's carried chain — bare re-raise keeps the original chain's
    /// head line, which pyrust's prepend path would otherwise double-count.
    /// An explicit `raise e` / `raise e.with_traceback(...)` clears it, so the
    /// prepend path (issue #2367's actual scope) runs for those.
    pub(crate) reraise_is_bare: bool,
    /// Compact hot-loop bitmask for active VM hooks (`VM_HOOK_TRACE`,
    /// `VM_HOOK_PROFILE`, `VM_HOOK_WEAKREF`). Packed into existing struct
    /// padding next to `reraise_is_bare` so `size_of::<Interpreter>()` stays
    /// unchanged; normal execution with all hooks disabled pays one byte test.
    pub(crate) vm_hooks_flags: u8,
}

/// Per-interpreter mutable state backing `sys` runtime getters/setters and
/// hooks (issue #2970).
#[derive(Debug)]
pub(crate) struct SysRuntimeState {
    pub(crate) switch_interval: f64,
    pub(crate) coroutine_origin_tracking_depth: usize,
    pub(crate) profile_func: Value,
    pub(crate) trace_func: Value,
    /// Re-entrancy guard: suppresses nested trace/profile dispatch while a
    /// trace or profile callback is executing (matching CPython 3.12).
    pub(crate) tracing_in_progress: bool,
    pub(crate) asyncgen_firstiter: Value,
    pub(crate) asyncgen_finalizer: Value,
    pub(crate) audit_hooks: Vec<Value>,
}

impl Default for SysRuntimeState {
    fn default() -> Self {
        Self {
            switch_interval: 0.005,
            coroutine_origin_tracking_depth: 0,
            profile_func: Value::none(),
            trace_func: Value::none(),
            tracing_in_progress: false,
            asyncgen_firstiter: Value::none(),
            asyncgen_finalizer: Value::none(),
            audit_hooks: Vec::new(),
        }
    }
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

/// Metadata for a script / module / `exec` / `eval` VM frame view (#3159).
/// Boxed inside [`FrameOwner::Script`] so hot function and generator frame views
/// stay compact (`size_of::<VmFrameView>() <= 56`).
pub(crate) struct ScriptFrameMeta {
    pub(crate) local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
    /// Root environment for this script frame so namespace-sensitive helpers
    /// distinguish an imported function's globals from the caller's script registers.
    pub(crate) env: EnvRef,
}

/// Compact ownership tag for [`VmFrameView`] (#3159).
///
/// Replaces redundant `local_index` / `nonlocal_names` / `env` / `is_class_method`
/// / `function` / `gen_frame` fields so pushing a function or generator view does
/// zero extra `Rc::clone`s beyond its single owner handle (zero `Rc::clone`s at
/// all for generators and owned-`Rc` trampolined calls).
pub(crate) enum FrameOwner {
    /// Ordinary or trampolined Python function frame (not a direct class method).
    Function(Rc<UserFunction>),
    /// Direct class-body method frame (`FnCode::is_class_method == true`).
    ClassMethod(Rc<UserFunction>),
    /// Active generator, coroutine, or async-generator frame.
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
    Generator(std::ptr::NonNull<GeneratorFrame>),
    /// Module / script / `exec` / `eval` frame.
    Script(Box<ScriptFrameMeta>),
    /// Class-body evaluation frame.
    Class(Rc<HashMap<String, crate::bytecode::Reg>>),
}

/// Snapshot of a VM frame's register file and owner metadata, used by
/// `globals()` / `locals()` / `vars()` / `sys._getframe()` / tracebacks to
/// surface names that live in registers rather than `env.values` (issue #389).
pub(crate) struct VmFrameView {
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
    pub(crate) regs_ptr: std::ptr::NonNull<Value>,
    pub(crate) regs_len: usize,
    /// Borrowed slice of this frame's indexed [`CellSlot`](pyrust_core::CellSlot)s
    /// (empty for Script frames and cell-free functions). Valid for the
    /// lifetime of this `VmFrameView` (popped before the backing `CellsBuf` /
    /// `UserFunction::closure_cells` / `GeneratorFrame::cells` is dropped).
    pub(crate) cells: CellSlice,
    /// Compact frame owner holding the frame's kind and metadata (#3159).
    pub(crate) owner: FrameOwner,
}

impl VmFrameView {
    #[inline(always)]
    pub(crate) fn function_frame(
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
        cells: CellSlice,
        function: Rc<UserFunction>,
        is_class_method: bool,
    ) -> Self {
        let owner = if is_class_method {
            FrameOwner::ClassMethod(function)
        } else {
            FrameOwner::Function(function)
        };
        Self {
            regs_ptr,
            regs_len,
            cells,
            owner,
        }
    }

    #[inline(always)]
    pub(crate) fn generator_frame(
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
        cells: CellSlice,
        gen_frame: std::ptr::NonNull<GeneratorFrame>,
    ) -> Self {
        Self {
            regs_ptr,
            regs_len,
            cells,
            owner: FrameOwner::Generator(gen_frame),
        }
    }

    #[inline]
    pub(crate) fn script_frame(
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
        env: EnvRef,
    ) -> Self {
        Self {
            regs_ptr,
            regs_len,
            cells: CellSlice::empty(),
            owner: FrameOwner::Script(Box::new(ScriptFrameMeta { local_index, env })),
        }
    }

    #[inline]
    pub(crate) fn class_frame(
        regs_ptr: std::ptr::NonNull<Value>,
        regs_len: usize,
        cells: CellSlice,
        local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
    ) -> Self {
        Self {
            regs_ptr,
            regs_len,
            cells,
            owner: FrameOwner::Class(local_index),
        }
    }

    #[inline]
    pub(crate) fn kind(&self) -> FrameKind {
        match &self.owner {
            FrameOwner::Function(_) | FrameOwner::ClassMethod(_) | FrameOwner::Generator(_) => {
                FrameKind::Function
            }
            FrameOwner::Script(_) => FrameKind::Script,
            FrameOwner::Class(_) => FrameKind::Class,
        }
    }

    #[inline]
    pub(crate) fn function(&self) -> Option<&Rc<UserFunction>> {
        match &self.owner {
            FrameOwner::Function(f) | FrameOwner::ClassMethod(f) => Some(f),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn into_function(self) -> Option<Rc<UserFunction>> {
        match self.owner {
            FrameOwner::Function(f) | FrameOwner::ClassMethod(f) => Some(f),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn gen_frame(&self) -> Option<std::ptr::NonNull<GeneratorFrame>> {
        match self.owner {
            FrameOwner::Generator(ptr) => Some(ptr),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn is_class_method(&self) -> bool {
        match self.owner {
            FrameOwner::ClassMethod(_) => true,
            FrameOwner::Generator(ptr) => {
                // SAFETY: `ptr` points to the heap-stable `GeneratorFrame`
                // while this view is on `vm_frame_views`.
                unsafe { ptr.as_ref().code.is_class_method }
            }
            _ => false,
        }
    }

    #[inline]
    pub(crate) fn local_index(&self) -> &HashMap<String, crate::bytecode::Reg> {
        match &self.owner {
            FrameOwner::Function(f) | FrameOwner::ClassMethod(f) => &f.local_index,
            FrameOwner::Generator(ptr) => {
                // SAFETY: `ptr` points to the heap-stable `GeneratorFrame`
                // while this view is on `vm_frame_views`.
                &unsafe { ptr.as_ref() }.function.local_index
            }
            FrameOwner::Script(meta) => &meta.local_index,
            FrameOwner::Class(local_index) => local_index,
        }
    }

    #[inline]
    pub(crate) fn nonlocal_names(&self) -> Option<&HashSet<String>> {
        let names = match &self.owner {
            FrameOwner::Function(f) | FrameOwner::ClassMethod(f) => &f.nonlocal_names,
            FrameOwner::Generator(ptr) => {
                // SAFETY: `ptr` points to the heap-stable `GeneratorFrame`
                // while this view is on `vm_frame_views`.
                &unsafe { ptr.as_ref() }.function.nonlocal_names
            }
            FrameOwner::Script(_) | FrameOwner::Class(_) => return None,
        };
        (!names.is_empty()).then_some(names.as_ref())
    }

    #[inline]
    pub(crate) fn env(&self) -> Option<&EnvRef> {
        match &self.owner {
            FrameOwner::Script(meta) => Some(&meta.env),
            _ => None,
        }
    }
}

pub(crate) struct VmFrameCache {
    pub(crate) object: Option<pyrust_core::WeakValueCache>,
    /// Strong handle retaining a live frame object while it carries a local
    /// `f_trace` callback so `f_trace` / `f_trace_lines` survive between lines
    /// even when the trace callback does not store `frame` externally.
    pub(crate) traced_frame: Option<Value>,
    /// Last line number and bytecode PC emitted by `sys.settrace` for this
    /// activation (cold; zero stack footprint in the VM hot loop).
    pub(crate) trace_last_line: u32,
    pub(crate) trace_last_pc: usize,
    /// Weak handle to this activation's function-locals mapping. Retaining
    /// only `frame.f_locals` must make later frame lookups reuse and refresh
    /// that dict without making the activation cache a new owner itself.
    pub(crate) function_locals: Option<std::rc::Weak<RefCell<PyDict>>>,
    /// Strong owner installed when `locals()` or argument-omitted `exec()`
    /// exposes this activation's function-locals mapping. The activation drops
    /// this cold cache on exit; generator activations retain it with the
    /// suspended frame.
    pub(crate) persistent_function_locals: Option<Rc<RefCell<PyDict>>>,
    /// Compiler-owned function-local keys synchronized into the persistent
    /// `f_locals` dict. Mapping-only keys inserted through `f_locals` are not
    /// recorded here and therefore survive later fastlocals refreshes.
    pub(crate) function_local_names: Vec<PyKey>,
    /// Unique per-activation liveness token used by deferred tracebacks to
    /// prove ABA-safely that a catching frame is still live on `vm_frame_views`
    /// when `e.__traceback__` materializes (issue #3029).
    pub(crate) live_token: Rc<()>,
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
            vm_frame_views: Vec::new(),
            tramp_arena: Vec::new(),
            tramp_stack: Vec::new(),
            gen_drive_stack: Vec::new(),
            call_arg_buf: Vec::new(),
            bound_method_pos_buf: Vec::new(),
            vm_frame_caches: None,
            int_max_str_digits: pyrust_core::INT_MAX_STR_DIGITS_DEFAULT,
            recursion_limit: DEFAULT_RECURSION_LIMIT,
            warnings_state: Rc::new(crate::builtin_modules::warnings::WarningsState::default()),
            sys_state: Rc::new(RefCell::new(SysRuntimeState::default())),
            cold: Box::default(),
            reraise_is_bare: false,
            vm_hooks_flags: 0,
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

/// Per-frame indexed cell-slot storage backing the VM (issues #452 / #2348).
/// Inline for 0–4 cells (zero heap allocation for virtually all closures).
pub(crate) type CellsBuf = smallvec::SmallVec<[pyrust_core::CellSlot; 4]>;

/// Non-owning slice view of a frame's [`CellSlot`](pyrust_core::CellSlot) array.
///
/// Mirrors [`RegSlice`]: allows the VM dispatch loop, `VmFrameView`,
/// `TrampFrame`, and `GenDriveFrame` to pass the active frame's cell slice
/// without lifetime gymnastics or cloning `SmallVec`s on every call/yield.
#[derive(Clone, Copy)]
pub(crate) struct CellSlice {
    ptr: *const pyrust_core::CellSlot,
    len: usize,
}

impl CellSlice {
    #[inline]
    pub(crate) const fn empty() -> Self {
        Self {
            ptr: std::ptr::NonNull::dangling().as_ptr(),
            len: 0,
        }
    }

    #[inline]
    pub(crate) fn from_slice(slice: &[pyrust_core::CellSlot]) -> Self {
        Self {
            ptr: slice.as_ptr(),
            len: slice.len(),
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn as_slice(&self) -> &[pyrust_core::CellSlot] {
        if self.len == 0 {
            &[]
        } else {
            // SAFETY: caller upholds backing lifetime while this CellSlice is live.
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }
}

/// Allocate a function frame's cell array: fresh empty `CellSlot`s for
/// `0..code.own_cell_count`, followed by clones of `closure_cells`.
#[inline]
pub(crate) fn allocate_frame_cells(
    code: &crate::bytecode::FnCode,
    closure_cells: &[pyrust_core::CellSlot],
) -> CellsBuf {
    let total = code.cell_slots.len();
    if total == 0 {
        return CellsBuf::new();
    }
    let own = code.own_cell_count as usize;
    let mut cells = CellsBuf::with_capacity(total);
    for _ in 0..own {
        cells.push(pyrust_core::CellSlot::new_empty());
    }
    cells.extend(closure_cells.iter().cloned());
    cells
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

/// The modes in which compiled code can be run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodeMode {
    /// Run as a sequence of statements; return `None`.
    Exec,
    /// Run as a single expression; return the expression's value.
    Eval,
    /// Run as a single interactive statement; expression results pass through
    /// `sys.displayhook`, returning `None`.
    Single,
}

/// Execution metadata attached to a `compile()`-created `code` object.
pub(crate) struct CodeState {
    pub(crate) code: Rc<FnCode>,
    pub(crate) mode: CodeMode,
    /// Local-index table used when compiling/running the code.
    pub(crate) local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
}

/// Construct a unified `code` object (`pyrust_builtins::code::CODE_OPS`)
/// wrapping a compiled [`FnCode`], so `compile()` results and `func.__code__`
/// share identical type, attributes, and methods (`co_lines`, `co_positions`,
/// `replace`, `dis`, etc.), with explicit `co_flags` bits (e.g.
/// `CO_FUTURE_ANNOTATIONS`, `CO_COROUTINE`).
pub(crate) fn value_code_object_with_flags(
    code: Rc<FnCode>,
    mode: CodeMode,
    local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
    extra_flags: i64,
) -> Value {
    let firstlineno = if code.first_lineno != 0 {
        code.first_lineno as i64
    } else {
        1
    };
    let (lines, positions) = build_code_lines_and_positions(&code, firstlineno);
    let linetable = pyrust_builtins::code::encode_linetable(&lines, firstlineno);
    let mut flags = extra_flags;
    if code.is_coroutine && !code.is_generator {
        flags |= pyrust_builtins::code::CO_COROUTINE;
    }
    let freevars: Vec<Value> = code
        .free_var_slots()
        .iter()
        .map(|slot| Value::string(slot.name.as_str()))
        .collect();
    let cellvars: Vec<Value> = code
        .own_cell_slots()
        .iter()
        .map(|slot| Value::string(slot.name.as_str()))
        .collect();
    let names: Vec<Value> = code
        .names
        .iter()
        .map(|sym| Value::string(sym.as_str()))
        .collect();
    let meta: Rc<dyn std::any::Any> = Rc::new(CodeState {
        code: Rc::clone(&code),
        mode,
        local_index,
    });
    pyrust_builtins::code::CodeBuild {
        name: code.qualname.to_string(),
        qualname: code.qualname.to_string(),
        argcount: 0,
        posonlyargcount: 0,
        kwonlyargcount: 0,
        nlocals: 0,
        stacksize: code.num_regs as i64,
        varnames: Vec::new(),
        flags,
        filename: code.filename.to_string(),
        firstlineno,
        consts: code.consts.clone(),
        names,
        freevars,
        cellvars,
        code_bytes: Vec::new(),
        linetable,
        exceptiontable: Vec::new(),
        lines,
        positions,
        raw_code: Some(Rc::clone(&code) as Rc<dyn std::any::Any>),
        exec_meta: Some(meta),
    }
    .build()
}

/// If `value` is a `compile()`-created code object, call `f` with its
/// `CodeState` and return the result.
pub(crate) fn with_code_state<R>(value: &Value, f: impl FnOnce(&CodeState) -> R) -> Option<R> {
    pyrust_builtins::code::with_exec_meta(value, |meta| meta.downcast_ref::<CodeState>().map(f))
        .flatten()
}

/// Forward a compile-time `SyntaxWarning` to the `warnings` module.
pub(crate) fn emit_syntax_warning_via_module(
    interp: &mut Interpreter,
    msg: &str,
    filename: &str,
    lineno: i64,
) -> Result<()> {
    crate::builtin_modules::warnings::emit_syntax_warning(interp, msg, filename, lineno)
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

pub(crate) mod builtin_args;

include!("interpreter/tests.rs");
