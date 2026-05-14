use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use indexmap::IndexMap;
use smallvec::smallvec;

use crate::ast::{AssignTarget, BinaryOp, CallArg, CmpOp, Expr, Stmt, UnaryOp};
use crate::bytecode::FnCode;
use crate::error::{PyError, Result};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{
    EnvRef, Environment, PyBigInt, PyClass, PyInstance, PyKey, PyModule, UserFunction,
    UserFunctionKind, UserFunctionParam, Value, ValueKind,
};

type ModuleCache = Rc<RefCell<HashMap<String, Value>>>;
type FnCache = HashMap<(u64, Vec<PyKey>), Value>;

const MAX_CALL_DEPTH: usize = 1000;
const ENV_POOL_MAX: usize = 64;
const SPEC_THRESHOLD: u8 = 8;

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
    spec_cache: HashMap<usize, SpecState>,
    /// Reusable argument buffer for VM Call instructions — avoids a per-call
    /// heap allocation in the common (non-recursive) case.
    call_arg_buf: Vec<ExpandedCallArg>,
    /// Reusable scratch buffer for building fn_cache probe keys — avoids a
    /// per-probe heap allocation in CallMemo's cache-hit path.
    key_scratch: Vec<crate::value::PyKey>,
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
    /// is still on the VM stack, i.e. inside the same `run_bytecode`
    /// call that pushed it.  The push/pop invariant in `program.rs`
    /// and `calls.rs` guarantees this.
    pub(crate) vm_frame_views: Vec<VmFrameView>,
}

/// Discriminator for `VmFrameView`: script-level (module-scope) vs.
/// function-level frames.  `globals()` and `locals()` need to tell
/// these apart — `globals()` always wants the script-level view,
/// `locals()` wants whichever is innermost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameKind {
    Script,
    Function,
}

/// Snapshot of a VM frame's register file and name index, used by
/// `globals()` / `locals()` / `vars()` to surface names that live in
/// registers rather than `env.values` (issue #389).
pub(crate) struct VmFrameView {
    pub(crate) kind: FrameKind,
    /// Raw pointer to the active frame's register slice. Valid only
    /// while the corresponding `run_bytecode` invocation is on the
    /// call stack — `Interpreter::vm_frame_views` is pushed/popped
    /// in lock-step with that lifetime by the caller.
    pub(crate) regs_ptr: *const Value,
    pub(crate) regs_len: usize,
    pub(crate) local_index: Rc<HashMap<String, crate::bytecode::Reg>>,
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
        Self {
            env,
            active_exception: None,
            handled_exc_stack: Vec::new(),
            script_dir: None,
            module_cache: Rc::new(RefCell::new(HashMap::new())),
            env_pool: Vec::new(),
            fn_cache: HashMap::new(),
            spec_cache: HashMap::new(),
            call_arg_buf: Vec::new(),
            key_scratch: Vec::new(),
            class_store_order: Vec::new(),
            vm_frame_views: Vec::new(),
        }
    }
}

include!("interpreter/runtime.rs");

include!("interpreter/helpers.rs");

pub(crate) mod builtin_args;

include!("interpreter/tests.rs");
