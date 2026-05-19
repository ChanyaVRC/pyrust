use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use indexmap::IndexMap;
use smallvec::smallvec;

use crate::ast::{AssignTarget, BinaryOp, Expr, Stmt, UnaryOp};
use crate::bytecode::FnCode;
use crate::error::{PyError, Result};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{
    EnvRef, Environment, PyBigInt, PyBigIntSign, PyClass, PyInstance, PyKey, PyModule, PyPow,
    PyToPrimitive, PyZero, UserFunction, UserFunctionKind, UserFunctionParam, Value, ValueKind,
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
/// Populated once at interpreter startup from `env` after
/// `install_exception_builtins` runs.  Lets the VM dispatch loop
/// construct `PyError::Class` errors without a per-raise env lookup.
pub(crate) struct ExcClasses(HashMap<&'static str, Rc<RefCell<PyClass>>>);

impl ExcClasses {
    fn new(env: &EnvRef) -> Self {
        let names: &[&'static str] = &[
            "BaseException",
            "Exception",
            "ArithmeticError",
            "OverflowError",
            "ZeroDivisionError",
            "FloatingPointError",
            "LookupError",
            "IndexError",
            "KeyError",
            "RuntimeError",
            "RecursionError",
            "NotImplementedError",
            "TypeError",
            "ValueError",
            "NameError",
            "AssertionError",
            "StopIteration",
            "AttributeError",
            "SyntaxError",
            "ImportError",
            "ModuleNotFoundError",
            "UnicodeError",
            "UnicodeEncodeError",
            "UnicodeDecodeError",
            "OSError",
            "FileNotFoundError",
            "SystemExit",
            "GeneratorExit",
            "KeyboardInterrupt",
        ];
        let mut map = HashMap::with_capacity(names.len());
        let module = env.borrow();
        for name in names {
            if let Some(v) = module.values.get(*name) {
                if let ValueKind::PyClass(cls) = v.kind() {
                    map.insert(*name, Rc::clone(cls));
                }
            }
        }
        ExcClasses(map)
    }

    /// Look up a built-in exception class by its `'static` name.
    /// Returns `None` only when `name` is not a registered built-in
    /// exception (should not happen in correct interpreter state).
    #[inline]
    pub(crate) fn get(&self, name: &'static str) -> Option<Rc<RefCell<PyClass>>> {
        self.0.get(name).map(Rc::clone)
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
    /// Raw pointer to the active frame's mutable register slice. Valid only
    /// while the corresponding `run_bytecode` invocation is on the
    /// call stack — `Interpreter::vm_frame_views` is pushed/popped
    /// in lock-step with that lifetime by the caller.  The pointer is
    /// `*mut` so that `assign_name`'s global-write path can update the
    /// corresponding fastlocal register when `StoreGlobal` fires from a
    /// nested scope (#520).
    pub(crate) regs_ptr: *mut Value,
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
        install_exception_builtins(&env);
        install_singleton_builtins(&env);
        // Snapshot built-in exception class Rcs so the VM dispatch loop can
        // construct `PyError::Class` errors without an env lookup per raise.
        let exc_classes = ExcClasses::new(&env);
        Self {
            env,
            active_exception: None,
            handled_exc_stack: Vec::new(),
            script_dir: None,
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            env_pool: Vec::new(),
            fn_cache: HashMap::new(),
            call_arg_buf: Vec::new(),
            key_scratch: Vec::new(),
            class_store_order: Vec::new(),
            vm_frame_views: Vec::new(),
            eq_in_progress: Vec::new(),
            exc_classes,
        }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

pub(crate) mod builtin_args;

include!("interpreter/tests.rs");
